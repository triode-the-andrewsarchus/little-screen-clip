//! The PNG backend: `PngPlan` lowers the resolved scene into
//! paint-ordered `PngElement`s in device space, `rasterize` paints them with
//! tiny-skia at a fixed 4x supersample and box-filters the result down, and
//! `compress` hands the straight-alpha raster to the owned encoder. The
//! per-index area queries exist so the session can charge the §13.2 raster
//! ledger from the plan before painting.
//!
//! The owned PNG encoder ADR-0004 describes — scanline filtering, deflate,
//! chunk assembly and the size-error vocabulary — is the sibling
//! `png_encoder`, which PDF's raster fallback shares; `paint` lowers
//! Rough drawings, freedraw outlines, labels and the Font Awesome placeholder
//! into tiny-skia paths and owns the shared frame-clip masks; `coverage` holds
//! the conservative device-space area estimators the ledger reads, which are
//! a pure function of the plan and never of the painted pixels.

mod canvas_image;
mod coverage;
mod guard;
mod paint;

use canvas_image::{
    device_draw_image_rect, paint_alpha8, scale_pixels_chain_pixels, AffineF, CanvasImageDraw,
    CcDecode, DecodeCache, DecodeCachePlanner, DecodeKey, LevelSource, PremulView, RectF,
};
use coverage::{
    conservative_coverage_area, conservative_fallback_coverage_area,
    conservative_freedraw_coverage_area, conservative_text_coverage_area, fallback_coverage_bounds,
    frame_clip_coverage_area, frame_clip_coverage_bounds, label_mask_coverage_area,
};
use guard::{GuardedDraw, GuardedMask};
use paint::{
    apply_theme_filter, composite_unmasked_raster_blocks, downsample_rgba, fallback_paths,
    finite_f32, label_path, paint_rough_drawing, solid_paint, svg_path, tiny_skia_freedraw_path,
    tiny_skia_path, PaintMasks, OFFICIAL_IMAGE_ERROR_BAN_PATH, OFFICIAL_IMAGE_ERROR_PATH,
    OFFICIAL_IMAGE_PATH,
};

use std::sync::Arc;

use crate::normalization::{NormalizedElementSource, NormalizedImageStatus};
use crate::portable_math::sin_cos;
use crate::{color::CanonicalColor, OutputFormat, RenderRequest};
use tiny_skia::{
    Color, FillRule, FilterQuality, IntSize, LineCap, LineJoin, Mask, PathBuilder, Pixmap,
    PixmapMut, PixmapPaint, PixmapRef, Rect, Stroke, StrokeDash, Transform,
};

use super::geometry::{Path, Point, FRAME_OUTLINE_STROKE_WIDTH};
use super::notices::{MandatoryNotices, NoticeError};
use super::plan::PlannedElement;
use super::png_encoder::{
    canonicalize_straight_alpha, compress_rgba, png_size_error, PngIdatStream,
};
use super::resolve::{
    decoration_fill_color, decoration_stroke_style, DarkModeImageFilter, FallbackColors,
    FallbackSource, ResolvedClip, ResolvedElement, ResolvedFrameOutline, ResolvedImageAsset,
    ResolvedImageSourceRect, ResolvedScene, ResolvedTransform, StrokeStyle,
};
use super::rough::{Drawing, Fill, FillKind};
use super::svg_filter_work::{self, FilterWork};
use super::text::{ResolvedLabel, ResolvedText};
use super::{freedraw, FormatEncodeError, SvgTreeCache};

/// Fixed supersampling matches browser-canvas edge coverage without host raster defaults.
const SUBPIXEL_SCALE: u32 = 4;

#[derive(Debug)]
pub(super) struct PngPlan {
    width: u32,
    height: u32,
    /// A side past `u32` saturated, so `width * height` is only a lower bound.
    saturated: bool,
    background: Option<[u8; 4]>,
    fallback_colors: FallbackColors,
    /// `drawImagePlaceholder`'s `#E7E7E7` rectangle, for the color mode.
    image_fallback_background: CanonicalColor,
    /// `drawImagePlaceholder`'s `#888` Font Awesome glyph, for the color mode.
    image_fallback_icon_color: CanonicalColor,
    svg_trees: SvgTreeCache,
    elements: Vec<PngElement>,
}

/// `drawImagePlaceholder`: the reference canvas fills the missing image's
/// rectangle `#E7E7E7` and draws the `#888` Font Awesome glyph over it. A
/// missing image is pending in the reference's image cache, so the canvas
/// keeps its theme filter for the placeholder and both colors pass through it.
const IMAGE_FALLBACK_BACKGROUND: CanonicalColor = CanonicalColor::from_rgb8(0xe7, 0xe7, 0xe7);
const IMAGE_FALLBACK_ICON_COLOR: CanonicalColor = CanonicalColor::from_rgb8(0x88, 0x88, 0x88);

/// One PNG plan element: the shared header in device space and the kind.
#[derive(Debug)]
struct PngElement {
    planned: PlannedElement<PngTransform, PngFrameClip, PngElementKind>,
}

/// What differs per element kind in the PNG plan; everything else an element
/// carries is the [`PlannedElement`] header. The PNG plan never writes the
/// header's link.
#[derive(Debug)]
enum PngElementKind {
    RoughOutline {
        source: NormalizedElementSource,
        drawing: Drawing,
        stroke_color: CanonicalColor,
        fill_color: Option<CanonicalColor>,
        stroke_width: f64,
        stroke_style: StrokeStyle,
        source_stroke_width: f64,
        background_color: CanonicalColor,
        label_mask: Option<PngLabelMask>,
        generated_label: Option<ResolvedText>,
        generated_label_color: Option<CanonicalColor>,
        generated_label_offset_y: f64,
    },
    Freedraw {
        source: NormalizedElementSource,
        path: freedraw::ProjectedPath,
        background_fill: Option<Fill>,
        color: CanonicalColor,
        background_color: Option<CanonicalColor>,
        source_stroke_width: f64,
    },
    Frame {
        source: NormalizedElementSource,
        name: Option<PngFrameName>,
        outline: Option<ResolvedFrameOutline>,
    },
    Text {
        source: NormalizedElementSource,
        text: ResolvedText,
        color: CanonicalColor,
    },
    Image {
        source: NormalizedElementSource,
        asset: Arc<ResolvedImageAsset>,
        /// The dark-mode filter the canvas applies to this image. The canvas
        /// resets its filter for a raster image, so only a nested-SVG image's
        /// rasterized pixels are lowered here.
        dark_mode_filter: Option<DarkModeImageFilter>,
        /// The rounded-corner clip of an image with a roundness record, in
        /// element space; without one the image clips to its rectangle.
        corner_clip: Option<Path>,
        source_rect: ResolvedImageSourceRect,
        scale: [i8; 2],
    },
    ImageFallback {
        source: NormalizedElementSource,
        status: NormalizedImageStatus,
    },
    Fallback {
        source: FallbackSource,
        label: Option<ResolvedLabel>,
    },
}

#[derive(Debug)]
struct PngFrameClip {
    frame_source_index: usize,
    path: Path,
    transform: PngTransform,
}

/// A frame's name at its own unrotated placement above the frame, painted at
/// full opacity and outside the frame's clip like the text element the
/// reference synthesizes for it.
#[derive(Debug)]
struct PngFrameName {
    label: ResolvedLabel,
    transform: PngTransform,
    color: CanonicalColor,
}

/// One frame clip mask the PNG plan builds once and shares across every
/// element that frame clips.
#[derive(Clone, Copy, Debug)]
pub(super) struct PngFrameClipMask {
    /// Plan index of the clipping frame's own element, when it is emitted.
    pub(super) frame_element_index: Option<usize>,
    /// Clipped conservative device-space area of the frame clip in canvas
    /// pixels: the alpha-mask generation cost `P` of the shared mask.
    pub(super) pixels: u64,
}

/// The supersampled layer one nested SVG image paints into: its offset and
/// size on the raster grid, and the transform resvg paints the document with.
#[derive(Clone, Copy, Debug)]
struct NestedSvgLayer {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    transform: Transform,
}

#[derive(Clone, Copy, Debug)]
struct PngLabelMask {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn project_frame_clips(
    clips: Vec<ResolvedClip>,
    scale: f64,
) -> Result<Vec<PngFrameClip>, FormatEncodeError> {
    let mut projected = Vec::new();
    projected
        .try_reserve_exact(clips.len())
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_plan",
            subject: "png_frame_clips",
        })?;
    for clip in clips {
        projected.push(PngFrameClip {
            frame_source_index: clip.frame_source_index,
            path: clip.path,
            transform: PngTransform::new(&clip.transform, scale),
        });
    }
    Ok(projected)
}

/// The identity of a frame clip set: the clipping frames, in order.
fn frame_clip_set_key(clips: &[PngFrameClip]) -> impl Iterator<Item = usize> + '_ {
    clips.iter().map(|clip| clip.frame_source_index)
}

/// Whether two clip sets are the same frame clip set: the same frames, in the
/// same order.
fn same_frame_clip_set(left: &[PngFrameClip], right: &[PngFrameClip]) -> bool {
    frame_clip_set_key(left).eq(frame_clip_set_key(right))
}

impl PngElement {
    /// A freshly planned element.
    const fn planned(planned: PlannedElement<PngTransform, PngFrameClip, PngElementKind>) -> Self {
        Self { planned }
    }

    fn painted_font_awesome_status(
        &self,
        surface_width: u32,
        surface_height: u32,
    ) -> Option<NormalizedImageStatus> {
        let PlannedElement {
            frame_clips,
            transform,
            opacity,
            width,
            height,
            kind: PngElementKind::ImageFallback { status, .. },
            ..
        } = &self.planned
        else {
            return None;
        };
        if !(*width > 0.0 && *height > 0.0 && *opacity > 0.0) {
            return None;
        }
        let (left, top, right, bottom) =
            fallback_coverage_bounds(*width, *height, *transform, surface_width, surface_height);
        let (clip_left, clip_top, clip_right, clip_bottom) =
            frame_clip_coverage_bounds(frame_clips, surface_width, surface_height)?;
        let left = left.max(clip_left);
        let top = top.max(clip_top);
        let right = right.min(clip_right);
        let bottom = bottom.min(clip_bottom);
        (left < right && top < bottom).then_some(*status)
    }

    /// The frame clips the element paints through the shared frame-clip mask.
    /// A compatibility fallback paints in its own layer and intersects its
    /// frame clips into that layer's clip instead, so it shares no mask.
    fn frame_clips(&self) -> &[PngFrameClip] {
        if matches!(self.planned.kind, PngElementKind::Fallback { .. }) {
            &[]
        } else {
            &self.planned.frame_clips
        }
    }

    fn label_mask(&self) -> Option<PngLabelMask> {
        match &self.planned.kind {
            PngElementKind::RoughOutline { label_mask, .. } => *label_mask,
            _ => None,
        }
    }

    /// The element-space clip an image or its placeholder paints through: the
    /// element rectangle, or an image's rounded-corner outline when it has a
    /// roundness record. The placeholder is never corner-clipped: the
    /// reference's `drawElementOnCanvas` image case calls `roundRect` and
    /// `clip` only before `drawImage`, not around `drawImagePlaceholder`. An
    /// axis-aligned image whose destination rectangle is snapped to pixel
    /// centres ([`PngElement::snapped_image_rect`]) needs no rectangle clip:
    /// the snapped rectangle is what it paints, and a rectangle mask at the
    /// fractional edge would cut the row the canvas paints whole (#162).
    fn image_clip(&self, raster_width: u32, raster_height: u32) -> Option<PngImageClip<'_>> {
        let corner_clip = match &self.planned.kind {
            PngElementKind::Image { corner_clip, .. } => corner_clip.as_ref(),
            PngElementKind::ImageFallback { .. } => None,
            _ => return None,
        };
        if corner_clip.is_none()
            && self
                .snapped_image_rect(raster_width, raster_height)
                .is_some()
        {
            return None;
        }
        Some(PngImageClip {
            width: self.planned.width,
            height: self.planned.height,
            corner_clip,
            transform: self.planned.transform,
        })
    }

    /// The device-pixel rectangle an axis-aligned raster image paints, or
    /// `None` when the image is a nested SVG, is rotated, is thinner than a
    /// device pixel, or lies entirely off the `raster_width` by
    /// `raster_height` canvas, in which case it paints with antialiased edges
    /// like any other shape.
    ///
    /// The reference canvas draws a raster image whose current transform
    /// keeps rectangles rectangles (rotation 0, any flip, any scale) without
    /// edge antialiasing, as Chromium's `drawImage` does for a destination at
    /// least one device pixel wide and high: Skia rounds each edge of the
    /// destination rectangle to `floor(edge + 0.5)`, so a pixel is painted if
    /// and only if its centre lies inside the rectangle, a centre on the left
    /// or top edge falling outside and one on the right or bottom edge inside.
    /// A fractional edge is therefore a whole row or column or none, never a
    /// blend, while the image is still sampled bilinearly at the exact
    /// fractional mapping (#162). The rectangle is in canvas pixels before the
    /// supersample; the painter scales it by the fixed multiplier, where its
    /// edges land on whole subpixels and the box filter leaves them whole. A
    /// nested-SVG image is not snapped: the canvas draws it as vectors whose
    /// own edge is antialiased and clips it to the destination with a second
    /// antialiased edge (see `clip_nested_svg_like_the_canvas`).
    fn snapped_image_rect(&self, raster_width: u32, raster_height: u32) -> Option<SnappedRect> {
        let PngElementKind::Image { asset, .. } = &self.planned.kind else {
            return None;
        };
        if asset.sanitized_svg.is_some() {
            return None;
        }
        let (left, top, right, bottom) = self.axis_aligned_device_rect()?;
        if !(right - left >= 1.0 && bottom - top >= 1.0) {
            return None;
        }
        let snap = |edge: f64, limit: u32| (edge + 0.5).floor().clamp(0.0, f64::from(limit)) as u32;
        let rect = SnappedRect {
            left: snap(left, raster_width),
            top: snap(top, raster_height),
            right: snap(right, raster_width),
            bottom: snap(bottom, raster_height),
        };
        (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
    }

    /// The element rectangle in canvas pixels, `(left, top, right, bottom)`,
    /// when the element's transform keeps rectangles rectangles (rotation 0,
    /// any flip, any scale); `None` for a rotated element.
    fn axis_aligned_device_rect(&self) -> Option<(f64, f64, f64, f64)> {
        let transform = self.planned.transform;
        if transform.b != 0.0 || transform.c != 0.0 {
            return None;
        }
        let origin = transform.map_point(Point { x: 0.0, y: 0.0 });
        let corner = transform.map_point(Point {
            x: self.planned.width,
            y: self.planned.height,
        });
        Some((
            origin.x.min(corner.x),
            origin.y.min(corner.y),
            origin.x.max(corner.x),
            origin.y.max(corner.y),
        ))
    }

    /// The supersampled layer an image element's nested SVG is painted into:
    /// its device bounds on the `raster_width` by `raster_height` grid,
    /// clamped to the canvas, and the transform resvg paints the document
    /// with. `None` when the element is no image or paints no layer pixel.
    fn nested_svg_layer(
        &self,
        raster_width: u32,
        raster_height: u32,
    ) -> Result<Option<NestedSvgLayer>, FormatEncodeError> {
        let PlannedElement {
            transform,
            width,
            height,
            kind: PngElementKind::Image {
                source_rect, scale, ..
            },
            ..
        } = &self.planned
        else {
            return Ok(None);
        };
        // A degenerate destination paints no pixels. Avoid inverting its
        // singular transform after a dimension default/recovery has
        // legitimately produced zero.
        if *width == 0.0 || *height == 0.0 {
            return Ok(None);
        }
        let corners = [
            Point { x: 0.0, y: 0.0 },
            Point { x: *width, y: 0.0 },
            Point {
                x: *width,
                y: *height,
            },
            Point { x: 0.0, y: *height },
        ]
        .map(|point| transform.map_point(point));
        let multiplier = f64::from(SUBPIXEL_SCALE);
        let left = (corners
            .iter()
            .map(|point| point.x)
            .fold(f64::INFINITY, f64::min)
            * multiplier)
            .floor()
            .clamp(0.0, f64::from(raster_width)) as u32;
        let top = (corners
            .iter()
            .map(|point| point.y)
            .fold(f64::INFINITY, f64::min)
            * multiplier)
            .floor()
            .clamp(0.0, f64::from(raster_height)) as u32;
        let right = (corners
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max)
            * multiplier)
            .ceil()
            .clamp(0.0, f64::from(raster_width)) as u32;
        let bottom = (corners
            .iter()
            .map(|point| point.y)
            .fold(f64::NEG_INFINITY, f64::max)
            * multiplier)
            .ceil()
            .clamp(0.0, f64::from(raster_height)) as u32;
        let Some(layer_width) = right.checked_sub(left).filter(|width| *width > 0) else {
            return Ok(None);
        };
        let Some(layer_height) = bottom.checked_sub(top).filter(|height| *height > 0) else {
            return Ok(None);
        };
        let raster_transform = transform.raster_image_transform_with_origin(
            SUBPIXEL_SCALE,
            *source_rect,
            *width,
            *height,
            *scale,
            left,
            top,
        )?;
        if !raster_transform.is_valid() {
            return Ok(None);
        }
        Ok(Some(NestedSvgLayer {
            left,
            top,
            width: layer_width,
            height: layer_height,
            transform: raster_transform,
        }))
    }

    /// The canvas `drawImage` call a raster image element issues (#198):
    /// the reference's `drawImage(image, crop..., 0, 0, width, height)` under
    /// its current transform, which Blink composes in binary64 (the element
    /// transform, then the flip `scale`) and hands to Skia as binary32, with
    /// the element opacity as the paint alpha. `None` for any other element
    /// and for a nested-SVG image.
    fn canvas_image_draw(&self) -> Option<Result<CanvasImageDraw, FormatEncodeError>> {
        let PngElementKind::Image {
            asset,
            source_rect,
            scale,
            ..
        } = &self.planned.kind
        else {
            return None;
        };
        if asset.sanitized_svg.is_some() {
            return None;
        }
        let PlannedElement {
            transform,
            opacity,
            width,
            height,
            ..
        } = &self.planned;
        let (width, height) = (*width, *height);
        let (sign_x, flip_x) = if scale[0] < 0 {
            (-1.0, width)
        } else {
            (1.0, 0.0)
        };
        let (sign_y, flip_y) = if scale[1] < 0 {
            (-1.0, height)
        } else {
            (1.0, 0.0)
        };
        let draw = || -> Result<CanvasImageDraw, FormatEncodeError> {
            Ok(CanvasImageDraw {
                ctm: AffineF {
                    a: finite_f32(transform.a * sign_x, "PNG image transform a")?,
                    b: finite_f32(transform.b * sign_x, "PNG image transform b")?,
                    c: finite_f32(transform.c * sign_y, "PNG image transform c")?,
                    d: finite_f32(transform.d * sign_y, "PNG image transform d")?,
                    e: finite_f32(
                        transform.e + transform.a * flip_x + transform.c * flip_y,
                        "PNG image transform e",
                    )?,
                    f: finite_f32(
                        transform.f + transform.b * flip_x + transform.d * flip_y,
                        "PNG image transform f",
                    )?,
                },
                src: RectF {
                    x: finite_f32(source_rect.x, "PNG image source x")?,
                    y: finite_f32(source_rect.y, "PNG image source y")?,
                    width: finite_f32(source_rect.width, "PNG image source width")?,
                    height: finite_f32(source_rect.height, "PNG image source height")?,
                },
                dst: RectF {
                    x: 0.0,
                    y: 0.0,
                    width: finite_f32(width, "PNG image width")?,
                    height: finite_f32(height, "PNG image height")?,
                },
                alpha8: paint_alpha8(*opacity),
            })
        };
        Some(draw())
    }

    /// Whether the element's device bounds miss the `surface_width` by
    /// `surface_height` canvas, so the canvas rejects its draw before
    /// decoding anything.
    fn misses_canvas(&self, surface_width: u32, surface_height: u32) -> bool {
        let corners = [
            Point { x: 0.0, y: 0.0 },
            Point {
                x: self.planned.width,
                y: 0.0,
            },
            Point {
                x: self.planned.width,
                y: self.planned.height,
            },
            Point {
                x: 0.0,
                y: self.planned.height,
            },
        ]
        .map(|point| self.planned.transform.map_point(point));
        let fold = |axis: fn(&Point) -> f64, pick: fn(f64, f64) -> f64, start: f64| {
            corners.iter().map(axis).fold(start, pick)
        };
        let left = fold(|point| point.x, f64::min, f64::INFINITY);
        let right = fold(|point| point.x, f64::max, f64::NEG_INFINITY);
        let top = fold(|point| point.y, f64::min, f64::INFINITY);
        let bottom = fold(|point| point.y, f64::max, f64::NEG_INFINITY);
        !(left < f64::from(surface_width)
            && right > 0.0
            && top < f64::from(surface_height)
            && bottom > 0.0)
    }
}

/// The cc decode level an image draw uses and where that level comes from.
#[derive(Clone, Copy, Debug)]
struct PlannedImageLevel {
    key: DecodeKey,
    decode: CcDecode,
    source: LevelSource,
}

/// One image level the render builds, as the resource ledger prices it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ImageLevelBuild {
    /// The plan index of the element whose draw builds the level.
    pub(super) index: usize,
    /// The level's pixels, kept for the rest of the render.
    pub(super) level_pixels: u64,
    /// The pixels the build reads: the decoded source rectangle or the cached
    /// level it scales from.
    pub(super) source_pixels: u64,
    /// The transient Skia mipmap chain the build allocates.
    pub(super) chain_pixels: u64,
}

/// Applies to a nested-SVG image's supersampled layer the two clips the
/// reference canvas applies when it draws an SVG image through `drawImage`
/// with an axis-aligned transform (Blink `SVGImage::DrawInternal`, #162):
///
/// - the SVG document's own viewport clip, antialiased, at the fractional
///   destination rectangle `rect` (canvas pixels): the layer is multiplied by
///   the destination's area coverage of each canvas pixel, so a fractional edge
///   row or column, whose content resvg already antialiases to its coverage,
///   carries the square of that coverage as the canvas does;
/// - `clipRect(ToEnclosingRect(dst_rect))`, a non-antialiased clip of the
///   destination rectangle enlarged to whole element-space pixels, `enclosing`
///   in canvas pixels, which Skia snaps to device pixels by rounding each edge
///   to `floor(edge + 0.5)`: a pixel whose centre lies at or left of the left
///   edge, or right of the right edge, is not painted at all.
///
/// `layer` is the supersampled layer whose top-left subpixel is `(layer_left,
/// layer_top)` on the surface and `multiplier` the supersample; every subpixel
/// of one canvas pixel takes that pixel's factor, so the box filter leaves the
/// product exact.
fn clip_nested_svg_like_the_canvas(
    layer: &mut Pixmap,
    layer_left: u32,
    layer_top: u32,
    rect: (f64, f64, f64, f64),
    enclosing: (f64, f64, f64, f64),
    multiplier: u32,
) {
    let (left, top, right, bottom) = rect;
    let coverage = |start: f64, end: f64, pixel: u32| -> f64 {
        let pixel = f64::from(pixel);
        (end.min(pixel + 1.0) - start.max(pixel)).clamp(0.0, 1.0)
    };
    let snapped = |edge: f64| (edge + 0.5).floor();
    let inside = |edge_start: f64, edge_end: f64, pixel: u32| -> bool {
        let pixel = f64::from(pixel);
        snapped(edge_start) <= pixel && pixel < snapped(edge_end)
    };
    let width = layer.width() as usize;
    let (layer_left, layer_top, multiplier) =
        (layer_left as usize, layer_top as usize, multiplier as usize);
    for (index, pixel) in layer.pixels_mut().iter_mut().enumerate() {
        let column = ((layer_left + index % width) / multiplier) as u32;
        let row = ((layer_top + index / width) / multiplier) as u32;
        let factor =
            if inside(enclosing.0, enclosing.2, column) && inside(enclosing.1, enclosing.3, row) {
                coverage(left, right, column) * coverage(top, bottom, row)
            } else {
                0.0
            };
        if factor >= 1.0 {
            continue;
        }
        let scale = |channel: u8| (f64::from(channel) * factor).round().clamp(0.0, 255.0) as u8;
        *pixel = tiny_skia::PremultipliedColorU8::from_rgba(
            scale(pixel.red()),
            scale(pixel.green()),
            scale(pixel.blue()),
            scale(pixel.alpha()),
        )
        .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
    }
}

/// The whole device pixels an axis-aligned image paints, in canvas pixels,
/// `left..right` by `top..bottom` (see [`PngElement::snapped_image_rect`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SnappedRect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

/// The clip `PaintMasks::paint_mask` rasterizes into an image's mask, in the
/// element's own space under `transform`: the rounded-corner outline when the
/// image has one, else the `width` by `height` rectangle.
struct PngImageClip<'a> {
    width: f64,
    height: f64,
    corner_clip: Option<&'a Path>,
    transform: PngTransform,
}

#[derive(Clone, Copy, Debug)]
struct PngTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    scale: f64,
}

impl PngTransform {
    fn new(transform: &ResolvedTransform, scale: f64) -> Self {
        let (sin, cos) = sin_cos(transform.angle_radians);
        Self {
            a: scale * cos,
            b: scale * sin,
            c: -scale * sin,
            d: scale * cos,
            e: scale
                * (transform.translation_x + transform.center_x - cos * transform.center_x
                    + sin * transform.center_y),
            f: scale
                * (transform.translation_y + transform.center_y
                    - sin * transform.center_x
                    - cos * transform.center_y),
            scale,
        }
    }

    fn raster_transform(self, multiplier: u32) -> Result<Transform, FormatEncodeError> {
        self.raster_transform_with_origin(multiplier, 0, 0)
    }

    fn raster_transform_with_origin(
        self,
        multiplier: u32,
        origin_x: u32,
        origin_y: u32,
    ) -> Result<Transform, FormatEncodeError> {
        let multiplier = f64::from(multiplier);
        let transform = Transform::from_row(
            finite_f32(self.a * multiplier, "PNG transform a")?,
            finite_f32(self.b * multiplier, "PNG transform b")?,
            finite_f32(self.c * multiplier, "PNG transform c")?,
            finite_f32(self.d * multiplier, "PNG transform d")?,
            finite_f32(self.e * multiplier - f64::from(origin_x), "PNG transform e")?,
            finite_f32(self.f * multiplier - f64::from(origin_y), "PNG transform f")?,
        );
        if !transform.is_valid() {
            return Err(FormatEncodeError::Encoding(
                "PNG transform is not invertible".to_owned(),
            ));
        }
        Ok(transform)
    }

    fn map_point(self, point: Point) -> Point {
        Point {
            x: self.a * point.x + self.c * point.y + self.e,
            y: self.b * point.x + self.d * point.y + self.f,
        }
    }

    fn with_local_offset(mut self, x: f64, y: f64) -> Self {
        self.e += self.a * x + self.c * y;
        self.f += self.b * x + self.d * y;
        self
    }

    /// Canvas glyph masks use quarter-pixel origins, not arbitrary fractional
    /// origins. Skia's SkPackedGlyphID has two subpixel bits and rounds with
    /// an eighth-pixel bias before truncation. The axis perpendicular to an
    /// axis-aligned baseline instead rounds to a whole pixel (#169).
    ///
    /// Apply this independently to each glyph after the device transform;
    /// shifting a whole rotated line cannot reproduce its different phases.
    /// Chromium 149 pins Skia 53348aa333da02b77c4b5797e2de722f5abde7d0:
    /// SkGlyph.h, prepare_for_direct_mask_drawing in SkGlyphRunPainter.cpp,
    /// and the 256-device-pixel path cutoff in SkStrikeSpec.cpp.
    fn canvas_text_path(
        self,
        label: &ResolvedLabel,
        font_size: f64,
        multiplier: u32,
    ) -> Result<Option<tiny_skia::Path>, FormatEncodeError> {
        let operations = &label.path.operations;
        let point_capacity = operations.len().checked_mul(3).ok_or_else(png_size_error)?;
        let mut builder = PathBuilder::with_capacity(operations.len(), point_capacity);
        let draw_as_path = font_size * font_size * (self.a * self.a + self.b * self.b)
            > 256.0 * 256.0
            || font_size * font_size * (self.c * self.c + self.d * self.d) > 256.0 * 256.0;
        // SkScalerContext's sk_relax classifies the font-cache device matrix:
        // admit each coefficient to SkScalar, multiply by 1024 in binary32,
        // then sk_float_round promotes to binary64 before adding 0.5/floor.
        // Font size and the raster supersample are not part of this matrix.
        // Only zero classification is needed; never round the actual geometry.
        let cache_axis_is_zero =
            |value: f64| (f64::from((value as f32) * 1024.0) + 0.5).floor() == 0.0;
        // Match computeAxisAlignmentForHText's horizontal-first precedence.
        let (x_grid, y_grid) = if cache_axis_is_zero(self.b) {
            (4.0, 1.0)
        } else if cache_axis_is_zero(self.a) {
            (1.0, 4.0)
        } else {
            (4.0, 4.0)
        };
        for glyph in &label.glyphs {
            let x = self.a * glyph.x + self.c * glyph.y + self.e;
            let y = self.b * glyph.x + self.d * glyph.y + self.f;
            let (dx, dy) = if draw_as_path {
                (0.0, 0.0)
            } else {
                (
                    (x * x_grid + 0.5).floor() / x_grid - x,
                    (y * y_grid + 0.5).floor() / y_grid - y,
                )
            };
            let outline = operations.get(glyph.outline.clone()).ok_or_else(|| {
                FormatEncodeError::Encoding("PNG glyph outline range is invalid".to_owned())
            })?;
            paint::append_label_path(&mut builder, outline, |x, y| {
                Ok((
                    finite_f32(
                        (self.a * x + self.c * y + self.e + dx) * f64::from(multiplier),
                        "PNG glyph x",
                    )?,
                    finite_f32(
                        (self.b * x + self.d * y + self.f + dy) * f64::from(multiplier),
                        "PNG glyph y",
                    )?,
                ))
            })?;
        }
        Ok(builder.finish())
    }

    fn raster_image_transform(
        self,
        multiplier: u32,
        source: ResolvedImageSourceRect,
        width: f64,
        height: f64,
        scale: [i8; 2],
    ) -> Result<Transform, FormatEncodeError> {
        self.raster_image_transform_with_origin(multiplier, source, width, height, scale, 0, 0)
    }

    #[allow(clippy::too_many_arguments)]
    fn raster_image_transform_with_origin(
        self,
        multiplier: u32,
        source: ResolvedImageSourceRect,
        width: f64,
        height: f64,
        scale: [i8; 2],
        origin_x: u32,
        origin_y: u32,
    ) -> Result<Transform, FormatEncodeError> {
        let base_scale_x = width / source.width;
        let base_scale_y = height / source.height;
        let (scale_x, offset_x) = if scale[0] < 0 {
            (-base_scale_x, width + source.x * base_scale_x)
        } else {
            (base_scale_x, -source.x * base_scale_x)
        };
        let (scale_y, offset_y) = if scale[1] < 0 {
            (-base_scale_y, height + source.y * base_scale_y)
        } else {
            (base_scale_y, -source.y * base_scale_y)
        };
        let multiplier = f64::from(multiplier);
        let transform = Transform::from_row(
            finite_f32(self.a * scale_x * multiplier, "PNG image transform a")?,
            finite_f32(self.b * scale_x * multiplier, "PNG image transform b")?,
            finite_f32(self.c * scale_y * multiplier, "PNG image transform c")?,
            finite_f32(self.d * scale_y * multiplier, "PNG image transform d")?,
            finite_f32(
                (self.a * offset_x + self.c * offset_y + self.e) * multiplier - f64::from(origin_x),
                "PNG image transform e",
            )?,
            finite_f32(
                (self.b * offset_x + self.d * offset_y + self.f) * multiplier - f64::from(origin_y),
                "PNG image transform f",
            )?,
        );
        // An image scaled to nothing has a singular transform; the canvas draws
        // nothing for it, and so do tiny-skia's fills. Only resvg needs the
        // check, which its caller makes.
        Ok(transform)
    }
}

impl PngPlan {
    pub(super) fn new(
        scene: ResolvedScene,
        request: &RenderRequest,
        svg_trees: &SvgTreeCache,
    ) -> Result<Self, FormatEncodeError> {
        let scale = scene.raster_or_pdf_scale(request.sizing());
        let width = pixel_dimension(scene.logical_width * scale, "width")?;
        let height = pixel_dimension(scene.logical_height * scale, "height")?;
        let saturated = width.is_none() || height.is_none();
        let width = width.unwrap_or(u32::MAX);
        let height = height.unwrap_or(u32::MAX);
        let background = scene
            .include_background
            .then(|| scene.background_color.rgba8());
        let mut elements = Vec::new();
        elements
            .try_reserve_exact(scene.elements.len())
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_plan",
                subject: "png_elements",
            })?;
        for element in scene.elements {
            match element {
                ResolvedElement::Outline(outline) => {
                    if outline.source.id().is_empty() {
                        return Err(FormatEncodeError::Encoding(format!(
                            "outline at {} has no normalized identity",
                            outline.source.pointer()
                        )));
                    }
                    elements.push(PngElement::planned(PlannedElement {
                        frame_clips: project_frame_clips(outline.frame_clips, scale)?,
                        transform: PngTransform::new(&outline.transform, scale),
                        opacity: outline.opacity,
                        link: outline.link,
                        width: outline.width,
                        height: outline.height,
                        kind: PngElementKind::RoughOutline {
                            source: outline.source,
                            drawing: outline.drawing,
                            stroke_color: outline.stroke_color,
                            fill_color: outline.fill_color,
                            stroke_width: outline.stroke_width,
                            stroke_style: outline.stroke_style,
                            source_stroke_width: outline.source_stroke_width,
                            background_color: outline.background_color,
                            label_mask: outline.label_mask.map(|mask| PngLabelMask {
                                x: mask.x * scale,
                                y: mask.y * scale,
                                width: mask.width * scale,
                                height: mask.height * scale,
                            }),
                            generated_label: outline.generated_label,
                            generated_label_color: outline.generated_label_color,
                            generated_label_offset_y: outline.generated_label_offset_y,
                        },
                    }));
                }
                ResolvedElement::Freedraw(freedraw) => {
                    elements.push(PngElement::planned(PlannedElement {
                        frame_clips: project_frame_clips(freedraw.frame_clips, scale)?,
                        transform: PngTransform::new(&freedraw.transform, scale),
                        opacity: freedraw.opacity,
                        link: freedraw.link,
                        width: freedraw.width,
                        height: freedraw.height,
                        kind: PngElementKind::Freedraw {
                            source: freedraw.source,
                            path: freedraw::project(freedraw.path)?,
                            background_fill: freedraw.background_fill,
                            color: freedraw.color,
                            background_color: freedraw.background_color,
                            source_stroke_width: freedraw.source_stroke_width,
                        },
                    }));
                }
                ResolvedElement::Frame(frame) => {
                    elements.push(PngElement::planned(PlannedElement {
                        frame_clips: project_frame_clips(frame.frame_clips, scale)?,
                        transform: PngTransform::new(&frame.transform, scale),
                        opacity: frame.opacity,
                        link: frame.link,
                        width: frame.width,
                        height: frame.height,
                        kind: PngElementKind::Frame {
                            source: frame.source,
                            name: frame.name.map(|name| PngFrameName {
                                transform: PngTransform::new(&name.transform, scale),
                                color: name.color,
                                label: name.label,
                            }),
                            outline: frame.outline,
                        },
                    }));
                }
                ResolvedElement::Text(text) => {
                    elements.push(PngElement::planned(PlannedElement {
                        frame_clips: project_frame_clips(text.frame_clips, scale)?,
                        transform: PngTransform::new(&text.transform, scale),
                        opacity: text.opacity,
                        link: text.link,
                        width: text.width,
                        height: text.height,
                        kind: PngElementKind::Text {
                            source: text.source,
                            text: text.text,
                            color: text.color,
                        },
                    }));
                }
                ResolvedElement::Image(image) => {
                    elements.push(PngElement::planned(PlannedElement {
                        frame_clips: project_frame_clips(image.frame_clips, scale)?,
                        transform: PngTransform::new(&image.transform, scale),
                        opacity: image.opacity,
                        link: image.link,
                        width: image.width,
                        height: image.height,
                        kind: PngElementKind::Image {
                            source: image.source,
                            asset: image.asset,
                            dark_mode_filter: image.dark_mode_filter,
                            corner_clip: image.corner_clip.map(|clip| clip.path),
                            source_rect: image.source_rect,
                            scale: image.scale,
                        },
                    }));
                }
                ResolvedElement::ImageFallback(image) => {
                    // The resolved placeholder carries no link: PNG writes
                    // none, and the reference canvas draws its image
                    // placeholder without one.
                    elements.push(PngElement::planned(PlannedElement {
                        frame_clips: project_frame_clips(image.frame_clips, scale)?,
                        transform: PngTransform::new(&image.transform, scale),
                        opacity: image.opacity,
                        link: None,
                        width: image.width,
                        height: image.height,
                        kind: PngElementKind::ImageFallback {
                            source: image.source,
                            status: image.status,
                        },
                    }));
                }
                ResolvedElement::Fallback(fallback) => {
                    elements.push(PngElement::planned(PlannedElement {
                        frame_clips: project_frame_clips(fallback.frame_clips, scale)?,
                        transform: PngTransform::new(&fallback.transform, scale),
                        opacity: fallback.opacity,
                        link: fallback.link,
                        width: fallback.width,
                        height: fallback.height,
                        kind: PngElementKind::Fallback {
                            source: fallback.source,
                            label: fallback.label,
                        },
                    }));
                }
            }
        }
        Ok(Self {
            width,
            height,
            saturated,
            background,
            svg_trees: svg_trees.clone(),
            fallback_colors: scene.fallback_colors,
            image_fallback_background: IMAGE_FALLBACK_BACKGROUND.for_mode(request.color_mode()),
            image_fallback_icon_color: IMAGE_FALLBACK_ICON_COLOR.for_mode(request.color_mode()),
            elements,
        })
    }

    /// Rasterizes the plan once and deflates the raster into the `IDAT`
    /// stream every artifact candidate is assembled from.
    pub(super) fn compress(&self) -> Result<PngIdatStream, FormatEncodeError> {
        let mut rgba = self.rasterize()?;
        canonicalize_straight_alpha(&mut rgba)?;
        compress_rgba(self.width, self.height, &rgba)
    }

    pub(super) fn mandatory_notices(&self) -> Result<Option<MandatoryNotices>, FormatEncodeError> {
        let has_image_fallback = self.elements.iter().any(|element| {
            element
                .painted_font_awesome_status(self.width, self.height)
                .is_some()
        });
        let has_error_image_fallback = self.elements.iter().any(|element| {
            element.painted_font_awesome_status(self.width, self.height)
                == Some(NormalizedImageStatus::Error)
        });
        let fallback_kinds: &[&str] = if has_error_image_fallback {
            &["image", "image-error"]
        } else {
            &["image"]
        };
        has_image_fallback
            .then(|| MandatoryNotices::for_fallbacks(fallback_kinds, OutputFormat::Png))
            .transpose()
            .map_err(|error| match error {
                NoticeError::Memory => FormatEncodeError::MemoryUnavailable {
                    subject_kind: "artifact_manifest",
                    subject: "font_awesome_notice_manifest",
                },
                NoticeError::InvalidProvenance => FormatEncodeError::Encoding(
                    "the mandatory Font Awesome notice could not be resolved".to_owned(),
                ),
            })
    }

    fn rasterize(&self) -> Result<Vec<u8>, FormatEncodeError> {
        let raster_width = self
            .width
            .checked_mul(SUBPIXEL_SCALE)
            .ok_or_else(png_size_error)?;
        let raster_height = self
            .height
            .checked_mul(SUBPIXEL_SCALE)
            .ok_or_else(png_size_error)?;
        let rgba_bytes = usize::try_from(raster_width)
            .ok()
            .and_then(|width| width.checked_mul(usize::try_from(raster_height).ok()?))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(png_size_error)?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(rgba_bytes)
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "raster_surface",
                subject: "png_main_surface",
            })?;
        rgba.resize(rgba_bytes, 0);

        {
            let mut surface = PixmapMut::from_bytes(&mut rgba, raster_width, raster_height)
                .ok_or_else(png_size_error)?;
            if let Some([red, green, blue, alpha]) = self.background {
                surface.fill(Color::from_rgba8(red, green, blue, alpha));
            }
            let mut masks = PaintMasks::new(raster_width, raster_height, SUBPIXEL_SCALE);
            let image_levels = self.plan_image_levels()?;
            let mut decode_cache = DecodeCache::default();
            for (index, element) in self.elements.iter().enumerate() {
                let paint_mask = masks.paint_mask(element)?;
                let PlannedElement {
                    frame_clips,
                    transform,
                    opacity,
                    link: _,
                    width,
                    height,
                    kind,
                } = &element.planned;
                match kind {
                    PngElementKind::RoughOutline {
                        source,
                        drawing,
                        stroke_color,
                        fill_color,
                        stroke_width,
                        stroke_style,
                        source_stroke_width,
                        background_color,
                        label_mask: _,
                        generated_label,
                        generated_label_color,
                        generated_label_offset_y,
                    } => {
                        if source.id().is_empty() || source.pointer().is_empty() {
                            return Err(FormatEncodeError::Encoding(
                                "PNG plan outline lost its source identity".to_owned(),
                            ));
                        }
                        let element_transform = *transform;
                        let transform = element_transform.raster_transform(SUBPIXEL_SCALE)?;
                        paint_rough_drawing(
                            &mut surface,
                            drawing,
                            transform,
                            *stroke_color,
                            *fill_color,
                            *stroke_width,
                            *stroke_style,
                            *source_stroke_width,
                            *opacity,
                            paint_mask,
                        )?;
                        for decoration in &drawing.decorations {
                            let fill =
                                decoration_fill_color(decoration, *stroke_color, *background_color);
                            let (decoration_style, decoration_stroke_width) =
                                decoration_stroke_style(decoration, *source_stroke_width);
                            paint_rough_drawing(
                                &mut surface,
                                &decoration.drawing,
                                transform,
                                *stroke_color,
                                Some(fill),
                                *stroke_width,
                                decoration_style,
                                decoration_stroke_width,
                                *opacity,
                                paint_mask,
                            )?;
                        }
                        if let (Some(label), Some(color)) = (generated_label, generated_label_color)
                        {
                            let paint = solid_paint(*color, 1.0)?;
                            let label_transform =
                                element_transform.with_local_offset(0.0, *generated_label_offset_y);
                            for line in &label.lines {
                                let Some(path) = label_transform.canvas_text_path(
                                    line,
                                    label.font_size,
                                    SUBPIXEL_SCALE,
                                )?
                                else {
                                    continue;
                                };
                                surface.guarded_fill_path(
                                    &path,
                                    &paint,
                                    FillRule::Winding,
                                    Transform::identity(),
                                    paint_mask,
                                );
                            }
                        }
                    }
                    PngElementKind::Freedraw {
                        source,
                        path,
                        background_fill,
                        color,
                        background_color,
                        source_stroke_width,
                    } => {
                        if source.id().is_empty() || source.pointer().is_empty() {
                            return Err(FormatEncodeError::Encoding(
                                "PNG freedraw lost its source identity".to_owned(),
                            ));
                        }
                        let raster_transform = transform.raster_transform(SUBPIXEL_SCALE)?;
                        if let (Some(fill), Some(background_color)) =
                            (background_fill, background_color)
                        {
                            let path = tiny_skia_path(&fill.path)?;
                            let paint = solid_paint(*background_color, *opacity)?;
                            match fill.kind {
                                FillKind::Solid => surface.guarded_fill_path(
                                    &path,
                                    &paint,
                                    FillRule::EvenOdd,
                                    raster_transform,
                                    paint_mask,
                                ),
                                FillKind::Sketch => {
                                    let stroke = Stroke {
                                        width: finite_f32(
                                            *source_stroke_width / 2.0,
                                            "PNG freedraw fill weight",
                                        )?,
                                        miter_limit: 4.0,
                                        line_cap: LineCap::Round,
                                        line_join: LineJoin::Round,
                                        dash: None,
                                    };
                                    surface.guarded_stroke_path(
                                        &path,
                                        &paint,
                                        &stroke,
                                        raster_transform,
                                        paint_mask,
                                    );
                                }
                            }
                        }
                        if path.is_empty() {
                            continue;
                        }
                        let path = tiny_skia_freedraw_path(path)?;
                        let paint = solid_paint(*color, *opacity)?;
                        surface.guarded_fill_path(
                            &path,
                            &paint,
                            FillRule::Winding,
                            raster_transform,
                            paint_mask,
                        );
                    }
                    PngElementKind::Frame {
                        source,
                        name,
                        outline,
                    } => {
                        if source.id().is_empty() || source.pointer().is_empty() {
                            return Err(FormatEncodeError::Encoding(
                                "PNG frame lost its source identity".to_owned(),
                            ));
                        }
                        if let Some(outline) = outline {
                            let path = tiny_skia_path(&outline.path)?;
                            // The reference's canvas path (`renderElement`)
                            // recolors a magic frame's outline by theme; its
                            // SVG path does not. Both are resolved for the mode.
                            let paint = solid_paint(outline.canvas_stroke_color, *opacity)?;
                            let stroke = Stroke {
                                width: FRAME_OUTLINE_STROKE_WIDTH as f32,
                                miter_limit: 4.0,
                                line_cap: LineCap::Round,
                                line_join: LineJoin::Round,
                                dash: None,
                            };
                            surface.guarded_stroke_path(
                                &path,
                                &paint,
                                &stroke,
                                transform.raster_transform(SUBPIXEL_SCALE)?,
                                paint_mask,
                            );
                        }
                        if let Some(name) = name {
                            if let Some(path) = name.transform.canvas_text_path(
                                &name.label,
                                super::text::FRAME_NAME_FONT_SIZE,
                                SUBPIXEL_SCALE,
                            )? {
                                let paint = solid_paint(name.color, 1.0)?;
                                surface.guarded_fill_path(
                                    &path,
                                    &paint,
                                    FillRule::Winding,
                                    Transform::identity(),
                                    None,
                                );
                            }
                        }
                    }
                    PngElementKind::Text {
                        source,
                        text,
                        color,
                    } => {
                        if source.id().is_empty() || source.pointer().is_empty() {
                            return Err(FormatEncodeError::Encoding(
                                "PNG text lost its source identity".to_owned(),
                            ));
                        }
                        let paint = solid_paint(*color, *opacity)?;
                        for line in &text.lines {
                            let Some(path) =
                                transform.canvas_text_path(line, text.font_size, SUBPIXEL_SCALE)?
                            else {
                                continue;
                            };
                            surface.guarded_fill_path(
                                &path,
                                &paint,
                                FillRule::Winding,
                                Transform::identity(),
                                paint_mask,
                            );
                        }
                    }
                    PngElementKind::Image {
                        source,
                        asset,
                        dark_mode_filter,
                        corner_clip: _,
                        source_rect,
                        scale,
                    } => {
                        // A degenerate destination paints no pixels. Avoid
                        // inverting its singular transform after a dimension
                        // default/recovery has legitimately produced zero.
                        if *width == 0.0 || *height == 0.0 {
                            continue;
                        }
                        if source.id().is_empty() || source.pointer().is_empty() {
                            return Err(FormatEncodeError::Encoding(
                                "PNG image lost its source identity".to_owned(),
                            ));
                        }
                        // An axis-aligned raster image paints the whole device
                        // pixels whose centres its destination rectangle
                        // covers, as the canvas's non-antialiased `drawImage`
                        // does (#162); a rotated one keeps antialiased edges.
                        // Either way the corner clip and frame clips stay
                        // antialiased masks.
                        let snapped = element.snapped_image_rect(self.width, self.height);
                        if let Some(tree) = self.svg_trees.get(&asset.identity).map(Arc::as_ref) {
                            let Some(NestedSvgLayer {
                                left,
                                top,
                                width: layer_width,
                                height: layer_height,
                                transform: raster_transform,
                            }) = element.nested_svg_layer(raster_width, raster_height)?
                            else {
                                continue;
                            };
                            let mut layer = Pixmap::new(layer_width, layer_height).ok_or(
                                FormatEncodeError::MemoryUnavailable {
                                    subject_kind: "raster_surface",
                                    subject: "png_nested_svg_surface",
                                },
                            )?;
                            // resvg paints through tiny-skia with no device-range
                            // guard; refuse a document whose painted extent would
                            // reach the range where its scan converter panics.
                            let root = tree.root();
                            let stroke_bounds = root.abs_stroke_bounding_box();
                            let layer_bounds = root.abs_layer_bounding_box();
                            let in_range = [
                                (
                                    stroke_bounds.left(),
                                    stroke_bounds.top(),
                                    stroke_bounds.right(),
                                    stroke_bounds.bottom(),
                                ),
                                (
                                    layer_bounds.left(),
                                    layer_bounds.top(),
                                    layer_bounds.right(),
                                    layer_bounds.bottom(),
                                ),
                            ]
                            .into_iter()
                            .all(|(left, top, right, bottom)| {
                                guard::within_device_bounds(
                                    left,
                                    top,
                                    right,
                                    bottom,
                                    0.0,
                                    raster_transform,
                                )
                            });
                            if !in_range {
                                return Err(FormatEncodeError::Encoding(
                                    "a nested SVG image paints beyond the PNG rasterizer's coordinate range"
                                        .to_owned(),
                                ));
                            }
                            resvg::render(tree, raster_transform, &mut layer.as_mut());
                            if *dark_mode_filter == Some(DarkModeImageFilter::NestedSvgImage) {
                                // The reference canvas keeps its theme filter
                                // for a nested-SVG image (`shouldResetImageFilter`
                                // resets it only for rasters), so the viewer's
                                // light-mode pixels pass through it here.
                                apply_theme_filter(&mut layer);
                            }
                            if let Some(rect) = element.axis_aligned_device_rect() {
                                // The canvas antialiases the nested document's
                                // viewport clip like its content edge, so a
                                // fractional edge carries its coverage squared,
                                // and clips the whole draw to the enclosing
                                // element-space rectangle snapped to device
                                // pixels (#162).
                                let enclosing = transform.map_point(Point {
                                    x: width.ceil(),
                                    y: height.ceil(),
                                });
                                clip_nested_svg_like_the_canvas(
                                    &mut layer,
                                    left,
                                    top,
                                    rect,
                                    (
                                        rect.0,
                                        rect.1,
                                        rect.0.max(enclosing.x),
                                        rect.1.max(enclosing.y),
                                    ),
                                    SUBPIXEL_SCALE,
                                );
                            }
                            surface.guarded_draw_pixmap(
                                i32::try_from(left).map_err(|_| png_size_error())?,
                                i32::try_from(top).map_err(|_| png_size_error())?,
                                layer.as_ref(),
                                &PixmapPaint {
                                    opacity: finite_f32(*opacity, "PNG image opacity")?,
                                    ..PixmapPaint::default()
                                },
                                Transform::identity(),
                                paint_mask,
                            );
                            continue;
                        }
                        let pixels = asset.premultiplied_rgba.as_deref().ok_or_else(|| {
                            FormatEncodeError::Encoding(
                                "PNG image has no normalized pixel resource".to_owned(),
                            )
                        })?;
                        let image = PremulView::new(asset.width, asset.height, pixels);
                        // The cc decode level the canvas samples (#198): a
                        // strongly minified draw samples a smaller level
                        // built from the source rectangle, reused across
                        // draws of the same image and rectangle.
                        let level = match image_levels.get(index).copied().flatten() {
                            Some(level) => Some((
                                level.decode,
                                decode_cache.level(
                                    level.key,
                                    image,
                                    &level.decode,
                                    level.source,
                                )?,
                            )),
                            None => None,
                        };
                        match snapped {
                            Some(_) => {
                                // The canvas's non-antialiased `drawImageRect`
                                // (#162, #165, #198): Skia's legacy sampler
                                // shades the whole device pixels whose centres
                                // the destination covers, once per canvas
                                // pixel, and each of those pixels then paints
                                // one constant block of subpixels, which the
                                // box filter returns unchanged. Paint alpha is
                                // part of the shaded pixel, and the blocks
                                // composite with the canvas's byte source-over
                                // unless a clip mask needs tiny-skia's masked
                                // fill. A cropped image is sampled from the
                                // whole decoded level, as the canvas samples
                                // it: the official evidence for
                                // `focused.image-codecs.v1` blends the pixels
                                // beside the crop into its edge rows.
                                let draw =
                                    element.canvas_image_draw().transpose()?.ok_or_else(|| {
                                        FormatEncodeError::Encoding(
                                            "PNG image has no canvas draw".to_owned(),
                                        )
                                    })?;
                                let (bitmap, draw) = match &level {
                                    Some((decode, level)) => (level.view(), draw.for_level(decode)),
                                    None => (image, draw),
                                };
                                let Some(painted) =
                                    device_draw_image_rect(bitmap, &draw, self.width, self.height)?
                                else {
                                    continue;
                                };
                                let layer = Pixmap::from_vec(
                                    painted.pixels,
                                    IntSize::from_wh(
                                        painted.right - painted.left,
                                        painted.bottom - painted.top,
                                    )
                                    .ok_or_else(png_size_error)?,
                                )
                                .ok_or_else(png_size_error)?;
                                if paint_mask.is_none() {
                                    composite_unmasked_raster_blocks(
                                        &mut surface,
                                        &layer,
                                        painted.left,
                                        painted.top,
                                        SUBPIXEL_SCALE,
                                    )?;
                                    continue;
                                }
                                let scaled = |edge: u32| {
                                    finite_f32(
                                        f64::from(edge) * f64::from(SUBPIXEL_SCALE),
                                        "PNG image destination edge",
                                    )
                                };
                                let destination = Rect::from_ltrb(
                                    scaled(painted.left)?,
                                    scaled(painted.top)?,
                                    scaled(painted.right)?,
                                    scaled(painted.bottom)?,
                                )
                                .ok_or_else(png_size_error)?;
                                let block_scale =
                                    finite_f32(f64::from(SUBPIXEL_SCALE), "PNG image block scale")?;
                                surface.guarded_fill_rect(
                                    destination,
                                    &tiny_skia::Paint {
                                        shader: tiny_skia::Pattern::new(
                                            layer.as_ref(),
                                            tiny_skia::SpreadMode::Pad,
                                            FilterQuality::Nearest,
                                            1.0,
                                            Transform::from_row(
                                                block_scale,
                                                0.0,
                                                0.0,
                                                block_scale,
                                                scaled(painted.left)?,
                                                scaled(painted.top)?,
                                            ),
                                        ),
                                        blend_mode: tiny_skia::BlendMode::SourceOver,
                                        anti_alias: false,
                                        force_hq_pipeline: false,
                                        colorspace: tiny_skia::ColorSpace::Linear,
                                    },
                                    Transform::identity(),
                                    paint_mask,
                                );
                            }
                            None => {
                                // A rotated image keeps tiny-skia's bilinear
                                // sampler with antialiased edges, drawing the
                                // cc decode level the canvas would sample.
                                let image_paint = PixmapPaint {
                                    opacity: finite_f32(*opacity, "PNG image opacity")?,
                                    quality: FilterQuality::Bilinear,
                                    ..PixmapPaint::default()
                                };
                                let raster_transform = transform.raster_image_transform(
                                    SUBPIXEL_SCALE,
                                    *source_rect,
                                    *width,
                                    *height,
                                    *scale,
                                )?;
                                match &level {
                                    Some((decode, level)) => {
                                        let (left, top, decoded_width, decoded_height) =
                                            decode.rect;
                                        let pixmap = PixmapRef::from_bytes(
                                            &level.data,
                                            level.width,
                                            level.height,
                                        )
                                        .ok_or_else(png_size_error)?;
                                        let level_transform = raster_transform
                                            .pre_translate(left as f32, top as f32)
                                            .pre_scale(
                                                finite_f32(
                                                    f64::from(decoded_width)
                                                        / f64::from(level.width),
                                                    "PNG image level scale",
                                                )?,
                                                finite_f32(
                                                    f64::from(decoded_height)
                                                        / f64::from(level.height),
                                                    "PNG image level scale",
                                                )?,
                                            );
                                        surface.guarded_draw_pixmap(
                                            0,
                                            0,
                                            pixmap,
                                            &image_paint,
                                            level_transform,
                                            paint_mask,
                                        );
                                    }
                                    None => {
                                        let pixmap = PixmapRef::from_bytes(
                                            pixels,
                                            asset.width,
                                            asset.height,
                                        )
                                        .ok_or_else(png_size_error)?;
                                        surface.guarded_draw_pixmap(
                                            0,
                                            0,
                                            pixmap,
                                            &image_paint,
                                            raster_transform,
                                            paint_mask,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    PngElementKind::ImageFallback { source, status } => {
                        if source.id().is_empty() || source.pointer().is_empty() {
                            return Err(FormatEncodeError::Encoding(
                                "PNG image fallback lost its source identity".to_owned(),
                            ));
                        }
                        let rect = Rect::from_xywh(
                            0.0,
                            0.0,
                            finite_f32(*width, "PNG image fallback width")?,
                            finite_f32(*height, "PNG image fallback height")?,
                        )
                        .ok_or_else(png_size_error)?;
                        surface.guarded_fill_path(
                            &PathBuilder::from_rect(rect),
                            &solid_paint(self.image_fallback_background, *opacity)?,
                            FillRule::Winding,
                            transform.raster_transform(SUBPIXEL_SCALE)?,
                            paint_mask,
                        );
                        let size = width.min(*height).min(width.min(*height) * 0.4).min(100.0);
                        if size <= 0.0 {
                            continue;
                        }
                        let icon_transform =
                            transform.raster_transform(SUBPIXEL_SCALE)?.pre_translate(
                                finite_f32((*width - size) / 2.0, "PNG image fallback icon x")?,
                                finite_f32((*height - size) / 2.0, "PNG image fallback icon y")?,
                            );
                        let icon_paint = solid_paint(self.image_fallback_icon_color, *opacity)?;
                        match status {
                            NormalizedImageStatus::Error => {
                                let icon_transform = icon_transform.pre_scale(
                                    finite_f32(size / 668.0, "PNG error icon scale")?,
                                    finite_f32(size / 668.0, "PNG error icon scale")?,
                                );
                                let image = svg_path(OFFICIAL_IMAGE_ERROR_PATH)?;
                                surface.guarded_fill_path(
                                    &image,
                                    &icon_paint,
                                    FillRule::Winding,
                                    icon_transform.pre_concat(Transform::from_row(
                                        0.81709, 0.0, 0.0, 0.81709, 124.825, 145.825,
                                    )),
                                    paint_mask,
                                );
                                let ban = svg_path(OFFICIAL_IMAGE_ERROR_BAN_PATH)?;
                                surface.guarded_fill_path(
                                    &ban,
                                    &icon_paint,
                                    FillRule::Winding,
                                    icon_transform.pre_concat(Transform::from_row(
                                        0.30366, 0.0, 0.0, 0.30366, 506.822, 60.065,
                                    )),
                                    paint_mask,
                                );
                            }
                            NormalizedImageStatus::Pending | NormalizedImageStatus::Saved => {
                                let icon = svg_path(OFFICIAL_IMAGE_PATH)?;
                                surface.guarded_fill_path(
                                    &icon,
                                    &icon_paint,
                                    FillRule::Winding,
                                    icon_transform.pre_scale(
                                        finite_f32(size / 512.0, "PNG image icon scale")?,
                                        finite_f32(size / 512.0, "PNG image icon scale")?,
                                    ),
                                    paint_mask,
                                );
                            }
                        }
                    }
                    PngElementKind::Fallback { source, label } => {
                        if source.pointer.is_empty() {
                            return Err(FormatEncodeError::Encoding(
                                "PNG fallback lost its source location".to_owned(),
                            ));
                        }
                        let (left, top, right, bottom) = fallback_coverage_bounds(
                            *width,
                            *height,
                            *transform,
                            self.width,
                            self.height,
                        );
                        let layer_width = right
                            .saturating_sub(left)
                            .checked_mul(SUBPIXEL_SCALE)
                            .ok_or_else(png_size_error)?;
                        let layer_height = bottom
                            .saturating_sub(top)
                            .checked_mul(SUBPIXEL_SCALE)
                            .ok_or_else(png_size_error)?;
                        if layer_width == 0 || layer_height == 0 {
                            continue;
                        }
                        let layer_x = left
                            .checked_mul(SUBPIXEL_SCALE)
                            .ok_or_else(png_size_error)?;
                        let layer_y = top.checked_mul(SUBPIXEL_SCALE).ok_or_else(png_size_error)?;
                        let transform = transform.raster_transform_with_origin(
                            SUBPIXEL_SCALE,
                            layer_x,
                            layer_y,
                        )?;
                        let (bounds, cross) = fallback_paths(*width, *height)?;
                        let mask_bytes = usize::try_from(layer_width)
                            .ok()
                            .and_then(|width| {
                                width.checked_mul(usize::try_from(layer_height).ok()?)
                            })
                            .ok_or_else(png_size_error)?;
                        let mut mask_data = Vec::new();
                        mask_data.try_reserve_exact(mask_bytes).map_err(|_| {
                            FormatEncodeError::MemoryUnavailable {
                                subject_kind: "raster_surface",
                                subject: "png_fallback_clip",
                            }
                        })?;
                        mask_data.resize(mask_bytes, 0);
                        let mask_size = IntSize::from_wh(layer_width, layer_height)
                            .ok_or_else(png_size_error)?;
                        let mut clip = Mask::from_vec(mask_data, mask_size).ok_or_else(|| {
                            FormatEncodeError::Encoding(
                                "PNG fallback clip dimensions are invalid".to_owned(),
                            )
                        })?;
                        clip.guarded_fill_path(&bounds, FillRule::Winding, true, transform);
                        for frame_clip in frame_clips {
                            let frame_bounds = tiny_skia_path(&frame_clip.path)?;
                            clip.guarded_intersect_path(
                                &frame_bounds,
                                FillRule::Winding,
                                true,
                                frame_clip.transform.raster_transform_with_origin(
                                    SUBPIXEL_SCALE,
                                    layer_x,
                                    layer_y,
                                )?,
                            );
                        }

                        let layer_rgba_bytes = usize::try_from(layer_width)
                            .ok()
                            .and_then(|width| {
                                width.checked_mul(usize::try_from(layer_height).ok()?)
                            })
                            .and_then(|pixels| pixels.checked_mul(4))
                            .ok_or_else(png_size_error)?;
                        let mut layer_bytes = Vec::new();
                        layer_bytes
                            .try_reserve_exact(layer_rgba_bytes)
                            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                                subject_kind: "raster_surface",
                                subject: "png_fallback_layer",
                            })?;
                        layer_bytes.resize(layer_rgba_bytes, 0);
                        let mut layer =
                            PixmapMut::from_bytes(&mut layer_bytes, layer_width, layer_height)
                                .ok_or_else(png_size_error)?;

                        let fill = solid_paint(self.fallback_colors.fill, 1.0)?;
                        layer.guarded_fill_path(
                            &bounds,
                            &fill,
                            FillRule::Winding,
                            transform,
                            Some(&clip),
                        );
                        if *width >= 2.0 && *height >= 2.0 {
                            let stroke_paint = solid_paint(self.fallback_colors.stroke, 1.0)?;
                            let dashed_stroke = Stroke {
                                width: 2.0,
                                miter_limit: 4.0,
                                line_cap: LineCap::Butt,
                                line_join: LineJoin::Miter,
                                dash: StrokeDash::new(vec![6.0, 4.0], 0.0),
                            };
                            layer.guarded_stroke_path(
                                &bounds,
                                &stroke_paint,
                                &dashed_stroke,
                                transform,
                                Some(&clip),
                            );
                            let cross_stroke = Stroke {
                                width: 2.0,
                                miter_limit: 4.0,
                                line_cap: LineCap::Butt,
                                line_join: LineJoin::Miter,
                                dash: None,
                            };
                            layer.guarded_stroke_path(
                                &cross,
                                &stroke_paint,
                                &cross_stroke,
                                transform,
                                Some(&clip),
                            );
                        }
                        if let Some(label) = label {
                            if let Some(label_path) = label_path(&label.path)? {
                                let label_paint = solid_paint(self.fallback_colors.label, 1.0)?;
                                layer.guarded_fill_path(
                                    &label_path,
                                    &label_paint,
                                    FillRule::Winding,
                                    transform,
                                    Some(&clip),
                                );
                            }
                        }
                        let layer_paint = PixmapPaint {
                            opacity: finite_f32(*opacity, "PNG fallback opacity")?,
                            ..PixmapPaint::default()
                        };
                        surface.guarded_draw_pixmap(
                            i32::try_from(layer_x).map_err(|_| png_size_error())?,
                            i32::try_from(layer_y).map_err(|_| png_size_error())?,
                            layer.as_ref(),
                            &layer_paint,
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
        }
        downsample_rgba(&rgba, raster_width, self.width, self.height, SUBPIXEL_SCALE)
    }

    /// The conservative device-space area of the element's own rectangle
    /// (`width` by `height` under its transform), clipped to the canvas.
    fn element_rect_coverage_area(&self, element: &PngElement) -> u64 {
        conservative_fallback_coverage_area(
            element.planned.width,
            element.planned.height,
            element.planned.transform,
            self.width,
            self.height,
        )
    }

    pub(super) fn vector_coverage_area(&self, index: usize) -> Option<u64> {
        self.elements.get(index).map(|element| {
            let planned = &element.planned;
            match &planned.kind {
                PngElementKind::RoughOutline {
                    drawing,
                    stroke_width,
                    ..
                } => conservative_coverage_area(
                    drawing,
                    planned.transform,
                    *stroke_width,
                    self.width,
                    self.height,
                ),
                PngElementKind::Freedraw {
                    path,
                    background_fill,
                    source_stroke_width,
                    ..
                } => conservative_freedraw_coverage_area(
                    path,
                    background_fill.as_ref(),
                    planned.transform,
                    *source_stroke_width,
                    self.width,
                    self.height,
                ),
                PngElementKind::Frame { .. } => 0,
                PngElementKind::Text { text, .. } => conservative_text_coverage_area(
                    text,
                    &planned.frame_clips,
                    planned.transform,
                    self.width,
                    self.height,
                ),
                PngElementKind::Image { .. } => 0,
                PngElementKind::ImageFallback { .. } | PngElementKind::Fallback { .. } => {
                    self.element_rect_coverage_area(element)
                }
            }
        })
    }

    /// The cc decode level of every painted raster image draw, indexed like
    /// the plan's elements, with the per-render decode cache's decisions in
    /// paint order (#198). A draw at level 0 samples the decoded image itself
    /// and has no entry; so does an element the session omitted or whose
    /// device bounds miss the canvas, which the canvas never decodes.
    fn plan_image_levels(&self) -> Result<Vec<Option<PlannedImageLevel>>, FormatEncodeError> {
        let mut planner = DecodeCachePlanner::default();
        let mut levels = Vec::new();
        levels.try_reserve_exact(self.elements.len()).map_err(|_| {
            FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_plan",
                subject: "png_image_levels",
            }
        })?;
        for element in &self.elements {
            let planned = match &element.planned.kind {
                PngElementKind::Image { asset, .. }
                    if element.planned.width != 0.0
                        && element.planned.height != 0.0
                        && !self.svg_trees.contains_key(&asset.identity)
                        && !element.misses_canvas(self.width, self.height) =>
                {
                    match element.canvas_image_draw().transpose()? {
                        Some(draw) => draw
                            .cc_decode(asset.width, asset.height)
                            .filter(|decode| decode.level > 0)
                            .map(|decode| {
                                let key = (asset.identity, decode.rect);
                                PlannedImageLevel {
                                    key,
                                    decode,
                                    source: planner.plan(
                                        key,
                                        decode.level_width,
                                        decode.level_height,
                                    ),
                                }
                            }),
                        None => None,
                    }
                }
                _ => None,
            };
            levels.push(planned);
        }
        Ok(levels)
    }

    /// The image levels the render builds, in paint order, for the resource
    /// ledger: a level reused from the decode cache builds nothing.
    pub(super) fn image_level_builds(&self) -> Result<Vec<ImageLevelBuild>, FormatEncodeError> {
        let mut builds = Vec::new();
        for (index, level) in self.plan_image_levels()?.into_iter().enumerate() {
            let Some(level) = level else {
                continue;
            };
            let (source_width, source_height) = match level.source {
                LevelSource::Hit => continue,
                LevelSource::Cached(width, height) => (width, height),
                LevelSource::Original => (level.decode.rect.2, level.decode.rect.3),
            };
            let (width, height) = (level.decode.level_width, level.decode.level_height);
            builds
                .try_reserve(1)
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "format_plan",
                    subject: "png_image_levels",
                })?;
            builds.push(ImageLevelBuild {
                index,
                level_pixels: u64::from(width) * u64::from(height),
                source_pixels: u64::from(source_width) * u64::from(source_height),
                chain_pixels: scale_pixels_chain_pixels(source_width, source_height, width, height),
            });
        }
        Ok(builds)
    }

    pub(super) fn image_projection_area(&self, index: usize) -> Option<Option<u64>> {
        self.elements
            .get(index)
            .map(|element| match &element.planned.kind {
                PngElementKind::Image { .. } => Some(self.element_rect_coverage_area(element)),
                _ => None,
            })
    }

    /// What resvg will spend on the filter effects of the nested SVG the
    /// element at `index` paints, measured on the layer `rasterize` hands it.
    /// The work counts the supersampled layer pixels resvg actually filters;
    /// the surfaces are in canvas pixels, the unit of the main surface, so a
    /// layer pixel counts one sixteenth. Empty for any other element. The
    /// walk stops past `budget` work units.
    pub(super) fn nested_svg_filter_work(
        &self,
        index: usize,
        budget: u64,
    ) -> Result<FilterWork, FormatEncodeError> {
        let Some(element) = self.elements.get(index) else {
            return Ok(FilterWork::default());
        };
        let PngElementKind::Image { asset, .. } = &element.planned.kind else {
            return Ok(FilterWork::default());
        };
        let Some(tree) = self.svg_trees.get(&asset.identity) else {
            return Ok(FilterWork::default());
        };
        let raster_width = self
            .width
            .checked_mul(SUBPIXEL_SCALE)
            .ok_or_else(png_size_error)?;
        let raster_height = self
            .height
            .checked_mul(SUBPIXEL_SCALE)
            .ok_or_else(png_size_error)?;
        let Some(layer) = element.nested_svg_layer(raster_width, raster_height)? else {
            return Ok(FilterWork::default());
        };
        let work =
            svg_filter_work::measure_tree(tree, layer.transform, layer.width, layer.height, budget);
        Ok(FilterWork {
            work_units: work.work_units,
            surface_pixels: work
                .surface_pixels
                .div_ceil(u64::from(SUBPIXEL_SCALE * SUBPIXEL_SCALE)),
        })
    }

    pub(super) fn fallback_surface_area(&self, index: usize) -> Option<Option<u64>> {
        self.elements
            .get(index)
            .map(|element| match &element.planned.kind {
                PngElementKind::Fallback { .. } => Some(self.element_rect_coverage_area(element)),
                PngElementKind::RoughOutline { .. }
                | PngElementKind::Freedraw { .. }
                | PngElementKind::Frame { .. }
                | PngElementKind::Text { .. }
                | PngElementKind::Image { .. }
                | PngElementKind::ImageFallback { .. } => None,
            })
    }

    /// The alpha mask an element needs beyond its shared frame clip mask, in
    /// canvas pixels: a bound-label exclusion or an image's own clip rectangle.
    /// `None` when the element paints through at most the shared frame mask.
    pub(super) fn paint_mask_surface_area(&self, index: usize) -> Option<Option<u64>> {
        self.elements
            .get(index)
            .map(|element| match &element.planned.kind {
                PngElementKind::RoughOutline {
                    label_mask: Some(label_mask),
                    ..
                } => Some(label_mask_coverage_area(
                    *label_mask,
                    self.width,
                    self.height,
                )),
                PngElementKind::Image { .. } | PngElementKind::ImageFallback { .. } => {
                    Some(self.element_rect_coverage_area(element))
                }
                _ => None,
            })
    }

    pub(super) fn has_paint_masks(&self) -> bool {
        self.elements.iter().any(|element| {
            matches!(
                element.planned.kind,
                PngElementKind::RoughOutline {
                    label_mask: Some(_),
                    ..
                } | PngElementKind::Image { .. }
                    | PngElementKind::ImageFallback { .. }
            )
        })
    }

    pub(super) fn maximum_paint_mask_surface_area(&self) -> u64 {
        self.elements
            .iter()
            .enumerate()
            .filter_map(|(index, _)| self.paint_mask_surface_area(index).flatten())
            .max()
            .unwrap_or(0)
    }

    /// The distinct frame clip masks this plan shares across clipped elements,
    /// in first-use element order. Each is charged once for the whole plan.
    pub(super) fn frame_clip_masks(&self) -> Result<Vec<PngFrameClipMask>, FormatEncodeError> {
        let mut representatives: Vec<&[PngFrameClip]> = Vec::new();
        for element in &self.elements {
            let clips = element.frame_clips();
            if clips.is_empty()
                || representatives
                    .iter()
                    .any(|known| same_frame_clip_set(known, clips))
            {
                continue;
            }
            representatives
                .try_reserve(1)
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "format_plan",
                    subject: "png_frame_clip_masks",
                })?;
            representatives.push(clips);
        }
        let mut masks = Vec::new();
        masks
            .try_reserve_exact(representatives.len())
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_plan",
                subject: "png_frame_clip_masks",
            })?;
        for clips in representatives {
            let frame_source_index = clips.first().map(|clip| clip.frame_source_index);
            masks.push(PngFrameClipMask {
                frame_element_index: self.elements.iter().position(|element| {
                    matches!(&element.planned.kind, PngElementKind::Frame { source, .. } if Some(source.source_index()) == frame_source_index)
                }),
                pixels: frame_clip_coverage_area(clips, self.width, self.height),
            });
        }
        Ok(masks)
    }

    pub(super) fn source_count(&self) -> usize {
        self.elements.len()
    }

    pub(super) fn source(&self, index: usize) -> Option<(Option<&str>, &str)> {
        self.elements
            .get(index)
            .map(|element| match &element.planned.kind {
                PngElementKind::RoughOutline { source, .. }
                | PngElementKind::Freedraw { source, .. }
                | PngElementKind::Frame { source, .. }
                | PngElementKind::Text { source, .. }
                | PngElementKind::Image { source, .. }
                | PngElementKind::ImageFallback { source, .. } => {
                    (Some(source.id()), source.pointer())
                }
                PngElementKind::Fallback { source, .. } => {
                    (source.id.as_deref(), source.pointer.as_str())
                }
            })
    }

    pub(super) const fn includes_background(&self) -> bool {
        self.background.is_some()
    }

    pub(super) const fn width(&self) -> u32 {
        self.width
    }

    #[cfg(test)]
    pub(super) const fn height(&self) -> u32 {
        self.height
    }

    /// The canvas area in pixels, or `None` when a side saturated and the
    /// area is known only to exceed every admissible surface.
    pub(super) const fn surface_pixels(&self) -> Option<u64> {
        if self.saturated {
            None
        } else {
            Some(self.width as u64 * self.height as u64)
        }
    }
}

/// A canvas side in whole pixels, or `None` for a side past `u32` (an
/// enormous scale), which the raster surface limit rejects as a resource
/// crossing. A side that truncates to zero (a tiny scale or maximum
/// dimension) is an encoding failure: the smallest valid sizing depends on
/// the scene, so the request cannot reject it.
fn pixel_dimension(value: f64, side: &str) -> Result<Option<u32>, FormatEncodeError> {
    if value >= (u32::MAX as f64) + 1.0 {
        return Ok(None);
    }
    if !value.is_finite() || value < 0.0 {
        return Err(FormatEncodeError::Encoding(
            "PNG dimensions must be finite positive 32-bit values".to_owned(),
        ));
    }
    if value < 1.0 {
        return Err(FormatEncodeError::Encoding(format!(
            "PNG {side} rounds to zero pixels at the requested size"
        )));
    }
    Ok(Some(value.trunc() as u32))
}
