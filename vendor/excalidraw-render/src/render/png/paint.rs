//! tiny-skia lowering for the PNG rasterizer: Rough drawings, freedraw
//! outlines, label glyph paths and the Font Awesome placeholder icons become
//! tiny-skia paths and paints here, the reference canvas's theme filter is
//! applied to rasterized nested-SVG layers, the supersampled surface is
//! box-filtered down, and `PaintMasks` keeps the shared frame-clip masks and
//! the per-element label and image clip masks one rasterization reuses.

use tiny_skia::{
    BlendMode, Color, ColorU8, FillRule, IntSize, LineCap, LineJoin, Mask, Paint, PathBuilder,
    PathStroker, Pixmap, PixmapMut, Rect, Shader, Stroke, StrokeDash, Transform,
};

use crate::color::{self, CanonicalColor};

use super::super::geometry::{Path, PathOperation};
use super::super::png_encoder::png_size_error;
use super::super::resolve::StrokeStyle;
use super::super::rough::{Drawing, FillKind};
use super::super::text::{LabelPath, LabelPathOperation};
use super::super::{freedraw, FormatEncodeError};
use super::coverage::frame_clip_coverage_bounds;
use super::guard::{GuardedDraw, GuardedMask};
use super::{frame_clip_set_key, PngElement, PngFrameClip, PngImageClip, PngLabelMask};

/// The canvas's unmasked N32 image blit: the already-resampled device pixel
/// fills one supersample block, using its byte source-over arithmetic. Keeping
/// the destination product at /256 (truncated) matters over nonwhite content.
pub(super) fn composite_unmasked_raster_blocks(
    surface: &mut PixmapMut<'_>,
    layer: &Pixmap,
    left: u32,
    top: u32,
    scale: u32,
) -> Result<(), FormatEncodeError> {
    let width = surface.width() as usize;
    let scale = scale as usize;
    let layer_width = layer.width() as usize;
    let data = surface.data_mut();
    for (y, row) in layer.data().chunks_exact(layer_width * 4).enumerate() {
        for sub_y in 0..scale {
            let start = (((top as usize + y) * scale + sub_y) * width + left as usize * scale) * 4;
            let target = data
                .get_mut(start..start + layer_width * scale * 4)
                .ok_or_else(|| {
                    FormatEncodeError::Encoding(
                        "PNG raster block bounds invariant failed".to_owned(),
                    )
                })?;
            for (source, block) in row
                .as_chunks::<4>()
                .0
                .iter()
                .zip(target.chunks_exact_mut(scale * 4))
            {
                let inverse = 256 - u16::from(source[3]);
                for pixel in block.as_chunks_mut::<4>().0.iter_mut() {
                    for (destination, source) in pixel.iter_mut().zip(source) {
                        *destination =
                            (u16::from(*source) + ((u16::from(*destination) * inverse) >> 8)) as u8;
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn downsample_rgba(
    source: &[u8],
    source_width: u32,
    width: u32,
    height: u32,
    scale: u32,
) -> Result<Vec<u8>, FormatEncodeError> {
    let output_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(png_size_error)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_bytes)
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "raster_surface",
            subject: "png_downsampled_surface",
        })?;
    output.resize(output_bytes, 0);
    let sample_count = scale.saturating_mul(scale);
    for y in 0..height {
        for x in 0..width {
            let mut sums = [0_u32; 4];
            for sample_y in 0..scale {
                for sample_x in 0..scale {
                    let source_pixel = u64::from(y * scale + sample_y)
                        .saturating_mul(u64::from(source_width))
                        .saturating_add(u64::from(x * scale + sample_x));
                    let source_offset = usize::try_from(source_pixel.saturating_mul(4))
                        .map_err(|_| png_size_error())?;
                    let source_end = source_offset.checked_add(4).ok_or_else(png_size_error)?;
                    let sample = source.get(source_offset..source_end).ok_or_else(|| {
                        FormatEncodeError::Encoding(
                            "PNG subpixel surface length invariant failed".to_owned(),
                        )
                    })?;
                    for (sum, channel) in sums.iter_mut().zip(sample) {
                        *sum += u32::from(*channel);
                    }
                }
            }
            let output_pixel = u64::from(y)
                .saturating_mul(u64::from(width))
                .saturating_add(u64::from(x));
            let output_offset =
                usize::try_from(output_pixel.saturating_mul(4)).map_err(|_| png_size_error())?;
            let output_end = output_offset.checked_add(4).ok_or_else(png_size_error)?;
            let pixel = output.get_mut(output_offset..output_end).ok_or_else(|| {
                FormatEncodeError::Encoding(
                    "PNG downsampled surface length invariant failed".to_owned(),
                )
            })?;
            for (channel, sum) in pixel.iter_mut().zip(sums) {
                *channel = u8::try_from((sum + sample_count / 2) / sample_count).map_err(|_| {
                    FormatEncodeError::Encoding(
                        "PNG downsampled channel invariant failed".to_owned(),
                    )
                })?;
            }
        }
    }
    Ok(output)
}

/// Lowers the reference canvas's theme filter into a rasterized layer: each
/// pixel's unpremultiplied color passes through the filter and is premultiplied
/// again, as the canvas filter reads and writes its surface; transparent pixels
/// have no color to filter.
pub(super) fn apply_theme_filter(layer: &mut Pixmap) {
    for pixel in layer.pixels_mut() {
        let alpha = pixel.alpha();
        if alpha == 0 {
            continue;
        }
        let color = pixel.demultiply();
        let [red, green, blue] = color::theme_filter([color.red(), color.green(), color.blue()]);
        *pixel = ColorU8::from_rgba(red, green, blue, alpha).premultiply();
    }
}

pub(super) fn tiny_skia_path(path: &Path) -> Result<tiny_skia::Path, FormatEncodeError> {
    drawable_path(path)?.ok_or_else(|| {
        FormatEncodeError::Encoding("PNG path contains no drawable operations".to_owned())
    })
}

/// `path` for tiny-skia, or `None` when it has no extent to draw: generated
/// geometry of a degenerate element, such as a fill pattern that places no
/// marks in a sliver-thin shape. The canvas draws nothing for such a path.
pub(super) fn drawable_path(path: &Path) -> Result<Option<tiny_skia::Path>, FormatEncodeError> {
    let points = path
        .operations
        .len()
        .checked_mul(3)
        .ok_or_else(png_size_error)?;
    let mut builder = PathBuilder::with_capacity(path.operations.len(), points);
    for operation in &path.operations {
        match operation {
            PathOperation::Move(point) => builder.move_to(
                finite_f32(point.x, "PNG path x")?,
                finite_f32(point.y, "PNG path y")?,
            ),
            PathOperation::Line(point) => builder.line_to(
                finite_f32(point.x, "PNG path x")?,
                finite_f32(point.y, "PNG path y")?,
            ),
            PathOperation::Cubic {
                control_1,
                control_2,
                end,
            } => builder.cubic_to(
                finite_f32(control_1.x, "PNG control x")?,
                finite_f32(control_1.y, "PNG control y")?,
                finite_f32(control_2.x, "PNG control x")?,
                finite_f32(control_2.y, "PNG control y")?,
                finite_f32(end.x, "PNG path x")?,
                finite_f32(end.y, "PNG path y")?,
            ),
        }
    }
    Ok(builder.finish())
}

pub(super) fn tiny_skia_freedraw_path(
    path: &freedraw::ProjectedPath,
) -> Result<tiny_skia::Path, FormatEncodeError> {
    let points = path
        .operations
        .len()
        .checked_mul(2)
        .ok_or_else(png_size_error)?;
    let mut builder = PathBuilder::with_capacity(path.operations.len(), points);
    for operation in &path.operations {
        match operation {
            freedraw::PathOperation::Move(point) => builder.move_to(
                finite_f32(point.x, "PNG freedraw x")?,
                finite_f32(point.y, "PNG freedraw y")?,
            ),
            freedraw::PathOperation::Quadratic { control, end } => builder.quad_to(
                finite_f32(control.x, "PNG freedraw control x")?,
                finite_f32(control.y, "PNG freedraw control y")?,
                finite_f32(end.x, "PNG freedraw x")?,
                finite_f32(end.y, "PNG freedraw y")?,
            ),
            freedraw::PathOperation::Line(point) => builder.line_to(
                finite_f32(point.x, "PNG freedraw x")?,
                finite_f32(point.y, "PNG freedraw y")?,
            ),
            freedraw::PathOperation::Close => builder.close(),
        }
    }
    builder.finish().ok_or_else(|| {
        FormatEncodeError::Encoding("PNG freedraw path contains no outline".to_owned())
    })
}

pub(super) fn fallback_paths(
    width: f64,
    height: f64,
) -> Result<(tiny_skia::Path, tiny_skia::Path), FormatEncodeError> {
    let width = finite_f32(width, "PNG fallback width")?;
    let height = finite_f32(height, "PNG fallback height")?;
    let mut bounds = PathBuilder::new();
    bounds.move_to(0.0, 0.0);
    bounds.line_to(width, 0.0);
    bounds.line_to(width, height);
    bounds.line_to(0.0, height);
    bounds.close();
    let bounds = bounds
        .finish()
        .ok_or_else(|| FormatEncodeError::Encoding("PNG fallback bounds are empty".to_owned()))?;
    let mut cross = PathBuilder::new();
    cross.move_to(0.0, 0.0);
    cross.line_to(width, height);
    cross.move_to(width, 0.0);
    cross.line_to(0.0, height);
    let cross = cross
        .finish()
        .ok_or_else(|| FormatEncodeError::Encoding("PNG fallback cross is empty".to_owned()))?;
    Ok((bounds, cross))
}

/// The raster path for one laid-out label line, or `None` when the line has no
/// outline to paint: an empty line, a whitespace-only line, or one whose glyphs
/// all lack outlines. Such a line still occupies its line step; it just paints
/// nothing, as in SVG and PDF (#111).
pub(super) fn label_path(path: &LabelPath) -> Result<Option<tiny_skia::Path>, FormatEncodeError> {
    if path.operations.is_empty() {
        return Ok(None);
    }
    let point_capacity = path
        .operations
        .len()
        .checked_mul(3)
        .ok_or_else(png_size_error)?;
    let mut builder = PathBuilder::with_capacity(path.operations.len(), point_capacity);
    append_label_path(&mut builder, &path.operations, |x, y| {
        Ok((finite_f32(x, "PNG label x")?, finite_f32(y, "PNG label y")?))
    })?;
    // `finish` declines a lone move, which has nothing to fill either.
    Ok(builder.finish())
}

pub(super) fn append_label_path(
    builder: &mut PathBuilder,
    operations: &[LabelPathOperation],
    point: impl Fn(f64, f64) -> Result<(f32, f32), FormatEncodeError>,
) -> Result<(), FormatEncodeError> {
    for operation in operations {
        match operation {
            LabelPathOperation::Move { x, y } => {
                let (x, y) = point(*x, *y)?;
                builder.move_to(x, y);
            }
            LabelPathOperation::Line { x, y } => {
                let (x, y) = point(*x, *y)?;
                builder.line_to(x, y);
            }
            LabelPathOperation::Quadratic {
                control_x,
                control_y,
                x,
                y,
            } => {
                let (cx, cy) = point(*control_x, *control_y)?;
                let (x, y) = point(*x, *y)?;
                builder.quad_to(cx, cy, x, y);
            }
            LabelPathOperation::Close => builder.close(),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_rough_drawing(
    surface: &mut PixmapMut<'_>,
    drawing: &Drawing,
    transform: Transform,
    stroke_color: CanonicalColor,
    fill_color: Option<CanonicalColor>,
    stroke_width: f64,
    stroke_style: StrokeStyle,
    source_stroke_width: f64,
    opacity: f64,
    mask: Option<&Mask>,
) -> Result<(), FormatEncodeError> {
    let fill = match (&drawing.fill, fill_color) {
        (Some(fill), Some(fill_color)) => {
            drawable_path(&fill.path)?.map(|path| (fill, fill_color, path))
        }
        _ => None,
    };
    if let Some((fill, fill_color, path)) = fill {
        let paint = solid_paint(fill_color, opacity)?;
        match fill.kind {
            FillKind::Solid => surface.guarded_fill_path(
                &path,
                &paint,
                if drawing.shape.canvas_uses_even_odd_fill() {
                    FillRule::EvenOdd
                } else {
                    FillRule::Winding
                },
                transform,
                mask,
            ),
            FillKind::Sketch => {
                let stroke = Stroke {
                    width: finite_f32(source_stroke_width / 2.0, "PNG fill weight")?,
                    miter_limit: 4.0,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    dash: None,
                };
                surface.guarded_stroke_path(&path, &paint, &stroke, transform, mask);
            }
        }
    }
    let Some(path) = drawable_path(&drawing.outline)? else {
        return Ok(());
    };
    let paint = solid_paint(stroke_color, opacity)?;
    let stroke = Stroke {
        width: finite_f32(stroke_width, "PNG stroke width")?,
        miter_limit: 4.0,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        dash: match stroke_style {
            StrokeStyle::Solid => None,
            StrokeStyle::Dashed => StrokeDash::new(
                vec![8.0, finite_f32(8.0 + source_stroke_width, "PNG dash gap")?],
                0.0,
            ),
            StrokeStyle::Dotted => StrokeDash::new(
                vec![1.5, finite_f32(6.0 + source_stroke_width, "PNG dot gap")?],
                0.0,
            ),
        },
    };
    // A positive width below f32's range narrows to zero, which the canvas
    // (Skia) draws as a hairline. tiny-skia's stroker has no outline for it;
    // its painter draws the hairline.
    if stroke.width == 0.0 {
        surface.guarded_stroke_path(&path, &paint, &stroke, transform, mask);
        return Ok(());
    }
    let resolution_scale = PathStroker::compute_resolution_scale(&transform);
    // `Stroke::dash` is inert for the low-level stroker: tiny-skia applies it by
    // dashing the path first, exactly as its own painter does. Stroking without
    // this step silently drops dashed and dotted stroke styles.
    let dashed_path;
    let path = if let Some(dash) = &stroke.dash {
        dashed_path = path.dash(dash, resolution_scale).ok_or_else(|| {
            FormatEncodeError::Encoding(
                "PNG raster adapter could not dash the Rough path".to_owned(),
            )
        })?;
        dashed_path
    } else {
        path
    };
    let path = path.stroke(&stroke, resolution_scale).ok_or_else(|| {
        FormatEncodeError::Encoding("PNG raster adapter could not stroke the Rough path".to_owned())
    })?;
    let path = path.transform(transform).ok_or_else(|| {
        FormatEncodeError::Encoding(
            "PNG raster adapter could not transform the stroked path".to_owned(),
        )
    })?;
    let bounds = path.bounds();
    if !(bounds.width() > 0.0 && bounds.height() > 0.0) {
        return Err(FormatEncodeError::Encoding(
            "PNG raster adapter produced empty stroked bounds".to_owned(),
        ));
    }
    surface.guarded_fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        mask,
    );
    Ok(())
}

/// The alpha masks one PNG rasterization reuses across elements.
///
/// tiny-skia composites a mask only when it matches the surface, so each mask
/// here is one full raster surface, allocated once per rasterization; what an
/// element does to them is bounded by the element, never the canvas, as the
/// resource ledger charges it:
///
/// - Every element a frame clips paints through that frame's mask. Each
///   distinct frame clip set is rasterized once, its covered rows are kept as a
///   stamp, and switching frames clears the previous stamp's rows and copies
///   the next one in, so a switch costs the two frame areas.
/// - A bound-label exclusion is punched into the frame mask, or into an
///   all-opaque mask when no frame clips the element, and the next element
///   restores it, so it costs the label rectangle.
/// - An image clip is rasterized into a mask that is clear outside the clip's
///   bounds, multiplied by the frame mask there, and the next image clip clears
///   those bounds again, so it costs the clip's bounds. The bytes are those of
///   intersecting the clip into a copy of the frame (or opaque) mask: outside
///   the bounds that intersection is zero, and inside it is the same product.
pub(super) struct PaintMasks {
    width: u32,
    height: u32,
    multiplier: u32,
    frame: Option<Mask>,
    stamps: Vec<FrameClipStamp>,
    current_stamp: Option<usize>,
    /// Opaque everywhere except a punched label rectangle.
    open: Option<Mask>,
    /// The label rectangle the previous element punched into a mask.
    hole: Option<(HoleTarget, RasterRect)>,
    /// Clear everywhere except `clip_bounds`.
    clip: Option<Mask>,
    clip_bounds: Option<RasterRect>,
}

/// The mask a label rectangle was punched into.
#[derive(Clone, Copy)]
enum HoleTarget {
    Frame,
    Open,
}

/// Raster pixels `left..right` by `top..bottom`, within the surface.
#[derive(Clone, Copy)]
struct RasterRect {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl RasterRect {
    fn rows(self, stride: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
        (self.top..self.bottom.max(self.top))
            .map(move |row| row * stride + self.left..row * stride + self.right.max(self.left))
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }
}

/// The rows of one frame clip set's mask that its clip covers, at raster
/// resolution.
struct FrameClipStamp {
    key: Vec<usize>,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl PaintMasks {
    pub(super) fn new(width: u32, height: u32, multiplier: u32) -> Self {
        Self {
            width,
            height,
            multiplier,
            frame: None,
            stamps: Vec::new(),
            current_stamp: None,
            open: None,
            hole: None,
            clip: None,
            clip_bounds: None,
        }
    }

    /// The mask `element` paints through, if any: its frame clip mask, that or
    /// an opaque mask with its bound label punched out, or its image clip.
    pub(super) fn paint_mask(
        &mut self,
        element: &PngElement,
    ) -> Result<Option<&Mask>, FormatEncodeError> {
        self.restore_hole();
        let clips = element.frame_clips();
        let label_mask = element.label_mask();
        let image_clip =
            element.image_clip(self.width / self.multiplier, self.height / self.multiplier);
        if clips.is_empty() && label_mask.is_none() && image_clip.is_none() {
            return Ok(None);
        }
        self.ensure_frame_mask(clips)?;
        let label = label_mask.map(|label| self.label_rect(label));
        if let Some(clip) = image_clip {
            return self
                .image_clip_mask(&clip, !clips.is_empty(), label)
                .map(Some);
        }
        let Some(label) = label else {
            return Ok(self.frame.as_ref());
        };
        let target = if clips.is_empty() {
            if self.open.is_none() {
                let mut open = self.blank_mask("png_paint_mask")?;
                open.data_mut().fill(255);
                self.open = Some(open);
            }
            HoleTarget::Open
        } else {
            HoleTarget::Frame
        };
        let stride = self.width as usize;
        let mask = match target {
            HoleTarget::Frame => self.frame.as_mut(),
            HoleTarget::Open => self.open.as_mut(),
        }
        .ok_or_else(png_size_error)?;
        for row in label.rows(stride) {
            mask.data_mut()[row].fill(0);
        }
        self.hole = Some((target, label));
        Ok(Some(mask))
    }

    /// Rasterizes `clip` into the clip mask, cleared from the previous one,
    /// times the frame mask when `framed` and without the `label` rectangle.
    fn image_clip_mask(
        &mut self,
        clip: &PngImageClip<'_>,
        framed: bool,
        label: Option<RasterRect>,
    ) -> Result<&Mask, FormatEncodeError> {
        if self.clip.is_none() {
            self.clip = Some(self.blank_mask("png_paint_mask")?);
        }
        let stride = self.width as usize;
        let mask = self.clip.as_mut().ok_or_else(png_size_error)?;
        if let Some(previous) = self.clip_bounds.take() {
            for row in previous.rows(stride) {
                mask.data_mut()[row].fill(0);
            }
        }
        let raster_transform = clip.transform.raster_transform(self.multiplier)?;
        let filled = match clip.corner_clip {
            Some(path) => mask.guarded_fill_path_bounds(
                &tiny_skia_path(path)?,
                FillRule::Winding,
                true,
                raster_transform,
            ),
            None => {
                let rect = Rect::from_xywh(
                    0.0,
                    0.0,
                    finite_f32(clip.width, "PNG image clip width")?,
                    finite_f32(clip.height, "PNG image clip height")?,
                )
                .ok_or_else(png_size_error)?;
                mask.guarded_fill_path_bounds(
                    &PathBuilder::from_rect(rect),
                    FillRule::Winding,
                    true,
                    raster_transform,
                )
            }
        };
        // The scan converter covers only the pixels the filled path's bounds
        // round out to; one more pixel each side absorbs the f32 translation
        // tiny-skia applies per tile on a surface wider than 8191 pixels.
        let Some(bounds) = filled.map(|bounds| {
            let clamp =
                |value: f32, side: u32| f64::from(value).clamp(0.0, f64::from(side)) as usize;
            RasterRect {
                left: clamp(bounds.left().floor() - 1.0, self.width),
                top: clamp(bounds.top().floor() - 1.0, self.height),
                right: clamp(bounds.right().ceil() + 1.0, self.width),
                bottom: clamp(bounds.bottom().ceil() + 1.0, self.height),
            }
        }) else {
            return Ok(mask);
        };
        self.clip_bounds = Some(bounds);
        if framed {
            let frame = self.frame.as_ref().ok_or_else(png_size_error)?;
            for row in bounds.rows(stride) {
                for (coverage, frame) in mask.data_mut()[row.clone()]
                    .iter_mut()
                    .zip(&frame.data()[row])
                {
                    *coverage = premultiply_u8(*frame, *coverage);
                }
            }
        }
        if let Some(label) = label {
            for row in label.intersect(bounds).rows(stride) {
                mask.data_mut()[row].fill(0);
            }
        }
        Ok(mask)
    }

    /// Restores the label rectangle the previous element punched out: opaque
    /// in the opaque mask, or the current frame stamp in the frame mask.
    fn restore_hole(&mut self) {
        let Some((target, hole)) = self.hole.take() else {
            return;
        };
        let stride = self.width as usize;
        match target {
            HoleTarget::Open => {
                if let Some(mask) = self.open.as_mut() {
                    for row in hole.rows(stride) {
                        mask.data_mut()[row].fill(255);
                    }
                }
            }
            HoleTarget::Frame => {
                let Some(mask) = self.frame.as_mut() else {
                    return;
                };
                let stamp = self.current_stamp.and_then(|index| self.stamps.get(index));
                let data = mask.data_mut();
                for y in hole.top..hole.bottom {
                    for x in hole.left..hole.right {
                        data[y * stride + x] = stamp.map_or(0, |stamp| stamp.value(x, y));
                    }
                }
            }
        }
    }

    /// Makes `self.frame` the mask of `clips`: the stamp of a frame clip set
    /// seen before is copied back in, a new set is rasterized once and stamped.
    fn ensure_frame_mask(&mut self, clips: &[PngFrameClip]) -> Result<(), FormatEncodeError> {
        if clips.is_empty() {
            return Ok(());
        }
        let wanted = self
            .stamps
            .iter()
            .position(|stamp| stamp.key.iter().copied().eq(frame_clip_set_key(clips)));
        if self.frame.is_some() && wanted.is_some() && wanted == self.current_stamp {
            return Ok(());
        }
        if self.frame.is_none() {
            self.frame = Some(self.blank_mask("png_frame_clip")?);
        }
        if let Some(previous) = self.current_stamp.take() {
            self.clear_stamp_rows(previous);
        }
        let index = match wanted {
            Some(index) => {
                self.copy_stamp_in(index);
                index
            }
            None => self.rasterize_stamp(clips)?,
        };
        self.current_stamp = Some(index);
        Ok(())
    }

    /// Rasterizes `clips` into the (cleared) frame mask and records the rows
    /// its coverage bounds span as a new stamp.
    fn rasterize_stamp(&mut self, clips: &[PngFrameClip]) -> Result<usize, FormatEncodeError> {
        // Coverage bounds are conservative in canvas pixels; the stamp scales
        // them to the raster and pads one raster pixel each side so that f32
        // edge rounding in the rasterizer can never leave coverage behind.
        let multiplier = usize::try_from(self.multiplier).map_err(|_| png_size_error())?;
        let raster_width = usize::try_from(self.width).map_err(|_| png_size_error())?;
        let raster_height = usize::try_from(self.height).map_err(|_| png_size_error())?;
        let (left, top, width, height) = frame_clip_coverage_bounds(
            clips,
            self.width / self.multiplier,
            self.height / self.multiplier,
        )
        .map_or((0, 0, 0, 0), |(left, top, right, bottom)| {
            let left = (left as usize * multiplier).saturating_sub(1);
            let top = (top as usize * multiplier).saturating_sub(1);
            let right = (right as usize * multiplier + 1).min(raster_width);
            let bottom = (bottom as usize * multiplier + 1).min(raster_height);
            (left, top, right - left, bottom - top)
        });
        let mask = self.frame.as_mut().ok_or_else(png_size_error)?;
        for (index, clip) in clips.iter().enumerate() {
            let path = tiny_skia_path(&clip.path)?;
            let transform = clip.transform.raster_transform(self.multiplier)?;
            if index == 0 {
                mask.guarded_fill_path(&path, FillRule::Winding, true, transform);
            } else {
                mask.guarded_intersect_path(&path, FillRule::Winding, true, transform);
            }
        }
        let memory = |_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "raster_surface",
            subject: "png_frame_clip_stamp",
        };
        let mut key = Vec::new();
        key.try_reserve_exact(clips.len()).map_err(memory)?;
        key.extend(frame_clip_set_key(clips));
        let mut data = Vec::new();
        data.try_reserve_exact(width.checked_mul(height).ok_or_else(png_size_error)?)
            .map_err(memory)?;
        for row in top..top + height {
            let start = row * raster_width + left;
            data.extend_from_slice(&mask.data()[start..start + width]);
        }
        self.stamps.try_reserve(1).map_err(memory)?;
        self.stamps.push(FrameClipStamp {
            key,
            left,
            top,
            width,
            height,
            data,
        });
        Ok(self.stamps.len() - 1)
    }

    fn clear_stamp_rows(&mut self, index: usize) {
        let Some((mask, stamp)) = self.frame.as_mut().zip(self.stamps.get(index)) else {
            return;
        };
        let stride = self.width as usize;
        let data = mask.data_mut();
        for row in stamp.top..stamp.top + stamp.height {
            let start = row * stride + stamp.left;
            data[start..start + stamp.width].fill(0);
        }
    }

    fn copy_stamp_in(&mut self, index: usize) {
        let Some((mask, stamp)) = self.frame.as_mut().zip(self.stamps.get(index)) else {
            return;
        };
        let stride = self.width as usize;
        let data = mask.data_mut();
        for (offset, row) in (stamp.top..stamp.top + stamp.height).enumerate() {
            let start = row * stride + stamp.left;
            let source = offset * stamp.width;
            data[start..start + stamp.width]
                .copy_from_slice(&stamp.data[source..source + stamp.width]);
        }
    }

    fn blank_mask(&self, subject: &'static str) -> Result<Mask, FormatEncodeError> {
        let mask_bytes = usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(usize::try_from(self.height).ok()?))
            .ok_or_else(png_size_error)?;
        let mut data = Vec::new();
        data.try_reserve_exact(mask_bytes)
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "raster_surface",
                subject,
            })?;
        data.resize(mask_bytes, 0);
        let size = IntSize::from_wh(self.width, self.height).ok_or_else(png_size_error)?;
        Mask::from_vec(data, size).ok_or_else(|| {
            FormatEncodeError::Encoding("PNG paint mask dimensions are invalid".to_owned())
        })
    }

    /// The raster pixels a bound-label exclusion clears: every pixel the label
    /// box touches, clamped to the surface.
    fn label_rect(&self, label: PngLabelMask) -> RasterRect {
        let multiplier = f64::from(self.multiplier);
        let left = (label.x * multiplier).floor().max(0.0) as u32;
        let top = (label.y * multiplier).floor().max(0.0) as u32;
        let right = ((label.x + label.width) * multiplier)
            .ceil()
            .clamp(0.0, f64::from(self.width)) as u32;
        let bottom = ((label.y + label.height) * multiplier)
            .ceil()
            .clamp(0.0, f64::from(self.height)) as u32;
        RasterRect {
            left: left.min(self.width) as usize,
            top: top.min(self.height) as usize,
            right: right as usize,
            bottom: bottom as usize,
        }
    }
}

impl FrameClipStamp {
    /// The frame mask's value at raster pixel (`x`, `y`): the stamped value
    /// inside the stamp, and clear outside it.
    fn value(&self, x: usize, y: usize) -> u8 {
        if (self.left..self.left + self.width).contains(&x)
            && (self.top..self.top + self.height).contains(&y)
        {
            self.data[(y - self.top) * self.width + x - self.left]
        } else {
            0
        }
    }
}

/// `a * b / 255`, rounded: the product `Mask::intersect_path` applies when it
/// intersects a coverage into a mask (tiny-skia's private `premultiply_u8`).
fn premultiply_u8(a: u8, b: u8) -> u8 {
    let product = u32::from(a) * u32::from(b) + 128;
    ((product + (product >> 8)) >> 8) as u8
}

pub(super) const OFFICIAL_IMAGE_PATH: &str = "M464 448H48c-26.51 0-48-21.49-48-48V112c0-26.51 21.49-48 48-48h416c26.51 0 48 21.49 48 48v288c0 26.51-21.49 48-48 48zM112 120c-30.928 0-56 25.072-56 56s25.072 56 56 56 56-25.072 56-56-25.072-56-56-56zM64 384h384V272l-87.515-87.515c-4.686-4.686-12.284-4.686-16.971 0L208 320l-55.515-55.515c-4.686-4.686-12.284-4.686-16.971 0L64 336v48z";
pub(super) const OFFICIAL_IMAGE_ERROR_PATH: &str = "M464 448H48c-26.51 0-48-21.49-48-48V112c0-26.51 21.49-48 48-48h416c26.51 0 48 21.49 48 48v288c0 26.51-21.49 48-48 48ZM112 120c-30.928 0-56 25.072-56 56s25.072 56 56 56 56-25.072 56-56-25.072-56-56-56ZM64 384h384V272l-87.515-87.515c-4.686-4.686-12.284-4.686-16.971 0L208 320l-55.515-55.515c-4.686-4.686-12.284-4.686-16.971 0L64 336v48Z";
pub(super) const OFFICIAL_IMAGE_ERROR_BAN_PATH: &str = "M256 8C119.034 8 8 119.033 8 256c0 136.967 111.034 248 248 248s248-111.034 248-248S392.967 8 256 8Zm130.108 117.892c65.448 65.448 70 165.481 20.677 235.637L150.47 105.216c70.204-49.356 170.226-44.735 235.638 20.676ZM125.892 386.108c-65.448-65.448-70-165.481-20.677-235.637L361.53 406.784c-70.203 49.356-170.226 44.736-235.638-20.676Z";

pub(super) fn svg_path(data: &str) -> Result<tiny_skia::Path, FormatEncodeError> {
    use svgtypes::PathSegment;

    let mut builder = PathBuilder::new();
    let mut current = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut cubic_control = None;
    let mut quadratic_control = None;
    let point = |abs: bool, x: f64, y: f64, current: (f64, f64)| {
        if abs {
            (x, y)
        } else {
            (current.0 + x, current.1 + y)
        }
    };
    for segment in svgtypes::PathParser::from(data) {
        let segment = segment.map_err(|_| {
            FormatEncodeError::Encoding("official image fallback path is invalid".to_owned())
        })?;
        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                current = point(abs, x, y, current);
                start = current;
                builder.move_to(
                    finite_f32(current.0, "PNG fallback move x")?,
                    finite_f32(current.1, "PNG fallback move y")?,
                );
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::LineTo { abs, x, y } => {
                current = point(abs, x, y, current);
                builder.line_to(
                    finite_f32(current.0, "PNG fallback line x")?,
                    finite_f32(current.1, "PNG fallback line y")?,
                );
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                current.0 = if abs { x } else { current.0 + x };
                builder.line_to(
                    finite_f32(current.0, "PNG fallback horizontal x")?,
                    finite_f32(current.1, "PNG fallback horizontal y")?,
                );
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::VerticalLineTo { abs, y } => {
                current.1 = if abs { y } else { current.1 + y };
                builder.line_to(
                    finite_f32(current.0, "PNG fallback vertical x")?,
                    finite_f32(current.1, "PNG fallback vertical y")?,
                );
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let control_1 = point(abs, x1, y1, current);
                let control_2 = point(abs, x2, y2, current);
                let end = point(abs, x, y, current);
                builder.cubic_to(
                    finite_f32(control_1.0, "PNG fallback cubic control x")?,
                    finite_f32(control_1.1, "PNG fallback cubic control y")?,
                    finite_f32(control_2.0, "PNG fallback cubic control x")?,
                    finite_f32(control_2.1, "PNG fallback cubic control y")?,
                    finite_f32(end.0, "PNG fallback cubic x")?,
                    finite_f32(end.1, "PNG fallback cubic y")?,
                );
                current = end;
                cubic_control = Some(control_2);
                quadratic_control = None;
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                let control_1 = cubic_control.map_or(current, |control: (f64, f64)| {
                    (2.0 * current.0 - control.0, 2.0 * current.1 - control.1)
                });
                let control_2 = point(abs, x2, y2, current);
                let end = point(abs, x, y, current);
                builder.cubic_to(
                    finite_f32(control_1.0, "PNG fallback smooth control x")?,
                    finite_f32(control_1.1, "PNG fallback smooth control y")?,
                    finite_f32(control_2.0, "PNG fallback smooth control x")?,
                    finite_f32(control_2.1, "PNG fallback smooth control y")?,
                    finite_f32(end.0, "PNG fallback smooth x")?,
                    finite_f32(end.1, "PNG fallback smooth y")?,
                );
                current = end;
                cubic_control = Some(control_2);
                quadratic_control = None;
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                let control = point(abs, x1, y1, current);
                let end = point(abs, x, y, current);
                builder.quad_to(
                    finite_f32(control.0, "PNG fallback quadratic control x")?,
                    finite_f32(control.1, "PNG fallback quadratic control y")?,
                    finite_f32(end.0, "PNG fallback quadratic x")?,
                    finite_f32(end.1, "PNG fallback quadratic y")?,
                );
                current = end;
                quadratic_control = Some(control);
                cubic_control = None;
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                let control = quadratic_control.map_or(current, |control: (f64, f64)| {
                    (2.0 * current.0 - control.0, 2.0 * current.1 - control.1)
                });
                let end = point(abs, x, y, current);
                builder.quad_to(
                    finite_f32(control.0, "PNG fallback smooth quadratic control x")?,
                    finite_f32(control.1, "PNG fallback smooth quadratic control y")?,
                    finite_f32(end.0, "PNG fallback smooth quadratic x")?,
                    finite_f32(end.1, "PNG fallback smooth quadratic y")?,
                );
                current = end;
                quadratic_control = Some(control);
                cubic_control = None;
            }
            PathSegment::ClosePath { .. } => {
                builder.close();
                current = start;
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::EllipticalArc { .. } => {
                return Err(FormatEncodeError::Encoding(
                    "official image fallback unexpectedly contains an arc".to_owned(),
                ));
            }
        }
    }
    builder.finish().ok_or_else(|| {
        FormatEncodeError::Encoding("official image fallback path is empty".to_owned())
    })
}

pub(super) fn solid_paint(
    color: CanonicalColor,
    opacity: f64,
) -> Result<Paint<'static>, FormatEncodeError> {
    let rgb = color.rgb8();
    let color = Color::from_rgba(
        f32::from(rgb[0]) / 255.0,
        f32::from(rgb[1]) / 255.0,
        f32::from(rgb[2]) / 255.0,
        finite_f32(opacity * color.alpha(), "PNG paint opacity")?,
    )
    .ok_or_else(|| FormatEncodeError::Encoding("PNG paint is outside RGBA range".to_owned()))?;
    Ok(Paint {
        shader: Shader::SolidColor(color),
        blend_mode: BlendMode::SourceOver,
        anti_alias: true,
        force_hq_pipeline: true,
        colorspace: tiny_skia::ColorSpace::Linear,
    })
}

pub(super) fn finite_f32(value: f64, subject: &str) -> Result<f32, FormatEncodeError> {
    let narrowed = value as f32;
    if !value.is_finite() || !narrowed.is_finite() {
        return Err(FormatEncodeError::Encoding(format!(
            "{subject} must be finite in the PNG raster domain"
        )));
    }
    Ok(narrowed)
}

#[cfg(test)]
mod tests {
    use crate::render::text::{LabelPath, LabelPathOperation};

    use super::label_path;

    #[test]
    fn a_line_with_no_outline_has_no_path_to_fill() {
        // Every label call site shares this helper, so an empty line, a
        // whitespace-only line, or a lone move paints nothing rather than
        // failing the artifact (#111).
        let empty = LabelPath {
            operations: Vec::new(),
        };
        assert!(matches!(label_path(&empty), Ok(None)));

        let lone_move = LabelPath {
            operations: vec![LabelPathOperation::Move { x: 1.0, y: 2.0 }],
        };
        assert!(matches!(label_path(&lone_move), Ok(None)));

        let outline = LabelPath {
            operations: vec![
                LabelPathOperation::Move { x: 0.0, y: 0.0 },
                LabelPathOperation::Line { x: 4.0, y: 0.0 },
                LabelPathOperation::Line { x: 4.0, y: 4.0 },
                LabelPathOperation::Close,
            ],
        };
        let Ok(Some(path)) = label_path(&outline) else {
            panic!("an outline has a path");
        };
        assert_eq!(path.len(), 4);
    }
}
