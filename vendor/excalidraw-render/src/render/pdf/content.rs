//! The PDF page content (ADR-0021), drawn on a Krilla surface in paint order:
//! per element, frame clips before the element's transform, Rough paths at the
//! reference's two-decimal precision, freedraw quadratics as cubics, text
//! glyph by glyph in the embedded Modified Versions, images through shared
//! image and nested-SVG graphics, and compatibility placeholders inline or in
//! a transparency group when they carry opacity.
//!
//! The page is a tagged PDF. Each source line of text is one `Span` marked
//! with its `/ActualText`, and the lines of one text element, label, frame
//! name or placeholder label form one `P`, in paint order. Everything else is
//! an artifact: the drawing's meaning is visual, and a figure without
//! alternative text would tell a reader nothing.

use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Size, Transform};
use std::sync::Arc;

use krilla::image::Image;
use krilla::num::NormalizedF32;
use krilla::paint::{Fill, FillRule, LineCap, LineJoin, Stroke, StrokeDash};
use krilla::surface::Surface;
use krilla::tagging::{Artifact, ArtifactType, ContentTag, SpanTag, Tag, TagGroup};

use crate::color::CanonicalColor;
use crate::portable_math::sin_cos;

use super::super::geometry::{
    Path as SourcePath, PathOperation, Point, FRAME_OUTLINE_STROKE_WIDTH,
};
use super::super::number_format::push_rough_number;
use super::super::plan::PlannedElement;
use super::super::resolve::{
    decoration_fill_color, decoration_stroke_style, FallbackColors, ResolvedClip,
    ResolvedTransform, StrokeStyle,
};
use super::super::rough::{Drawing, FillKind};
use super::super::text::{ResolvedLabel, ResolvedText};
use super::super::{freedraw, FormatEncodeError};
use super::fonts::{draw_glyphs, finite_f32, PdfFont, PdfFonts};
use super::{visible_rect, PdfElementKind, PdfPlan, POINTS_PER_LOGICAL_PIXEL};

/// The resources an image element draws: one shared raster image per image
/// resource and the tree of each nested-SVG resource, in resource order.
///
/// A nested SVG is drawn where each element shows it rather than once as a
/// shared form: viewers disagree on the space a pattern inside a reused form
/// refers to, and PDF.js then reverses the SVG's gradients.
pub(super) struct PdfImages {
    pub(super) raster: Vec<Image>,
    pub(super) svg: Vec<Arc<usvg::Tree>>,
}

impl PdfPlan {
    /// Draws the page and returns its tag groups in reading (paint) order.
    pub(super) fn draw_page(
        &self,
        surface: &mut Surface<'_>,
        fonts: &PdfFonts,
        images: &PdfImages,
    ) -> Result<Vec<TagGroup>, FormatEncodeError> {
        let mut page = Page {
            surface,
            fonts,
            tags: Vec::new(),
            pixels_per_unit: self.scale,
            depth: 0,
        };
        let result = self.draw_elements(&mut page, images);
        page.unwind();
        result.map(|()| page.tags)
    }

    fn draw_elements(
        &self,
        page: &mut Page<'_, '_, '_>,
        images: &PdfImages,
    ) -> Result<(), FormatEncodeError> {
        let image_resources = self.image_resources()?;
        let points = self.scale * POINTS_PER_LOGICAL_PIXEL;
        page.push_transform(points, 0.0, 0.0, points, 0.0, 0.0)?;
        if let Some(background) = self.background {
            page.artifact(|page| {
                let path = rect_path(0.0, 0.0, self.logical_width, self.logical_height)?;
                page.fill(&path, background, background.alpha(), FillRule::NonZero)
            })?;
        }
        for element in &self.elements {
            let PlannedElement {
                frame_clips,
                transform,
                opacity,
                link: _,
                width,
                height,
                kind,
            } = element;
            match kind {
                PdfElementKind::RoughOutline {
                    source,
                    drawing,
                    stroke_color,
                    fill_color,
                    stroke_width,
                    stroke_style,
                    source_stroke_width,
                    background_color,
                    label_mask,
                    generated_label,
                    generated_label_color,
                    generated_label_offset_y,
                } => {
                    if source.id().is_empty() || source.pointer().is_empty() {
                        return Err(FormatEncodeError::Encoding(
                            "PDF plan outline lost its source identity".to_owned(),
                        ));
                    }
                    let mut pushes = page.push_frame_clips(frame_clips)?;
                    if let Some(mask) = label_mask {
                        let mut builder = PathBuilder::new();
                        push_rect(
                            &mut builder,
                            0.0,
                            0.0,
                            self.logical_width,
                            self.logical_height,
                        )?;
                        push_rect(&mut builder, mask.x, mask.y, mask.width, mask.height)?;
                        let path = finish_path(builder, "PDF arrow label mask")?;
                        page.push_clip(&path, FillRule::EvenOdd);
                        pushes += 1;
                    }
                    page.push_resolved_transform(transform)?;
                    pushes += 1;
                    page.artifact(|page| {
                        page.draw_drawing(
                            drawing,
                            *stroke_color,
                            *fill_color,
                            *stroke_width,
                            *stroke_style,
                            *source_stroke_width,
                            *opacity,
                        )?;
                        for decoration in &drawing.decorations {
                            let fill =
                                decoration_fill_color(decoration, *stroke_color, *background_color);
                            let (decoration_style, decoration_stroke_width) =
                                decoration_stroke_style(decoration, *source_stroke_width);
                            page.draw_drawing(
                                &decoration.drawing,
                                *stroke_color,
                                Some(fill),
                                *stroke_width,
                                decoration_style,
                                decoration_stroke_width,
                                *opacity,
                            )?;
                        }
                        Ok(())
                    })?;
                    if let (Some(label), Some(color)) = (generated_label, generated_label_color) {
                        page.push_transform(1.0, 0.0, 0.0, 1.0, 0.0, *generated_label_offset_y)?;
                        page.text(label, *color, color.alpha())?;
                        page.pop(1);
                    }
                    page.pop(pushes);
                }
                PdfElementKind::Freedraw {
                    source,
                    path,
                    background_fill,
                    color,
                    background_color,
                    source_stroke_width,
                } => {
                    if source.id().is_empty() || source.pointer().is_empty() {
                        return Err(FormatEncodeError::Encoding(
                            "PDF freedraw lost its source identity".to_owned(),
                        ));
                    }
                    let pushes = page.push_frame_clips(frame_clips)? + 1;
                    page.push_resolved_transform(transform)?;
                    page.artifact(|page| {
                        if let (Some(fill), Some(background_color)) =
                            (background_fill, background_color)
                        {
                            let fill_opacity = *opacity * background_color.alpha();
                            match fill.kind {
                                FillKind::Solid => page.fill(
                                    &rough_path(&fill.path)?,
                                    *background_color,
                                    fill_opacity,
                                    FillRule::EvenOdd,
                                )?,
                                FillKind::Sketch => page.sketch_stroke(
                                    &fill.path,
                                    *background_color,
                                    fill_opacity,
                                    *source_stroke_width / 2.0,
                                )?,
                            }
                        }
                        if !path.is_empty() {
                            page.fill(
                                &freedraw_path(path)?,
                                *color,
                                *opacity * color.alpha(),
                                FillRule::NonZero,
                            )?;
                        }
                        Ok(())
                    })?;
                    page.pop(pushes);
                }
                PdfElementKind::Frame {
                    source,
                    name,
                    outline,
                } => {
                    if source.id().is_empty() || source.pointer().is_empty() {
                        return Err(FormatEncodeError::Encoding(
                            "PDF frame lost its source identity".to_owned(),
                        ));
                    }
                    // The name is its own text element: unrotated, unclipped
                    // and fully opaque, painted before the frame.
                    if let Some(name) = name {
                        page.push_resolved_transform(&name.transform)?;
                        page.label(&name.label, name.color)?;
                        page.pop(1);
                    }
                    let pushes = page.push_frame_clips(frame_clips)? + 1;
                    page.push_resolved_transform(transform)?;
                    if let Some(outline) = outline {
                        // PDF follows the reference SVG's outline color, which
                        // does not recolor magic frames.
                        page.artifact(|page| {
                            page.stroke(
                                &rough_path(&outline.path)?,
                                outline.stroke_color,
                                *opacity,
                                FRAME_OUTLINE_STROKE_WIDTH,
                                LineCap::Round,
                                LineJoin::Round,
                                None,
                            )
                        })?;
                    }
                    page.pop(pushes);
                }
                PdfElementKind::Text {
                    source,
                    text,
                    color,
                } => {
                    if source.id().is_empty() || source.pointer().is_empty() {
                        return Err(FormatEncodeError::Encoding(
                            "PDF text lost its source identity".to_owned(),
                        ));
                    }
                    let pushes = page.push_frame_clips(frame_clips)? + 1;
                    page.push_resolved_transform(transform)?;
                    page.text(text, *color, *opacity * color.alpha())?;
                    page.pop(pushes);
                }
                PdfElementKind::Image {
                    source_order: _,
                    source,
                    asset,
                    dark_mode_filter: _,
                    corner_clip,
                    source_rect: _,
                    projection,
                    scale,
                } => {
                    if source.id().is_empty() || source.pointer().is_empty() {
                        return Err(FormatEncodeError::Encoding(
                            "PDF image lost its source identity".to_owned(),
                        ));
                    }
                    enum Resource<'a> {
                        Raster(&'a Image),
                        Svg(&'a usvg::Tree),
                    }
                    let resource = if asset.sanitized_svg.is_some() {
                        self.svg_resources
                            .iter()
                            .position(|candidate| {
                                if asset.svg_effects.is_empty() {
                                    candidate.asset.identity == asset.identity
                                } else {
                                    candidate.source_index == source.source_index()
                                }
                            })
                            .and_then(|index| images.svg.get(index))
                            .map(|tree| Resource::Svg(tree))
                    } else {
                        image_resources
                            .iter()
                            .position(|candidate| candidate.asset.identity == asset.identity)
                            .and_then(|index| images.raster.get(index))
                            .map(Resource::Raster)
                    };
                    let resource = resource.ok_or_else(|| {
                        FormatEncodeError::Encoding(
                            "PDF image resource has no semantic owner".to_owned(),
                        )
                    })?;
                    let mut pushes = page.push_frame_clips(frame_clips)? + 1;
                    page.push_resolved_transform(transform)?;
                    if *opacity != 1.0 {
                        page.push_opacity(*opacity, "PDF image opacity")?;
                        pushes += 1;
                    }
                    // The element-space clips precede the content transform,
                    // so a rounded corner stays at the element's corner
                    // however the source is mapped into it. Image clips keep
                    // full precision (spec 8.6): the rough two-decimal
                    // rounding does not apply to them, and only the viewport
                    // carries the reference's integer rounding.
                    if let Some(path) = corner_clip {
                        let path = exact_path(path, |x, y| (x, y), "PDF image clip")?;
                        page.push_clip(&path, FillRule::NonZero);
                        pushes += 1;
                    }
                    let (content_x, content_y, content_width, content_height) =
                        projection.content_rect();
                    // A cropped image overflows the region the reference
                    // leaves visible; an uncropped one fills its viewport, so
                    // the clip only restates the image's own bounds and yields
                    // to a corner clip.
                    if projection.crop.is_some() || corner_clip.is_none() {
                        let (x, y, visible_width, visible_height) =
                            visible_rect(projection, *width, *height, *scale);
                        let path = rect_path(x, y, visible_width, visible_height)?;
                        page.push_clip(&path, FillRule::NonZero);
                        pushes += 1;
                    }
                    // The asset fills the content rectangle, mirrored about the
                    // element's unrounded center when flipped.
                    let (scale_x, offset_x) = if scale[0] < 0 {
                        (-content_width, *width - content_x)
                    } else {
                        (content_width, content_x)
                    };
                    let (scale_y, offset_y) = if scale[1] < 0 {
                        (-content_height, *height - content_y)
                    } else {
                        (content_height, content_y)
                    };
                    page.artifact(|page| match resource {
                        Resource::Raster(image) => {
                            page.push_transform(scale_x, 0.0, 0.0, scale_y, offset_x, offset_y)?;
                            page.surface.draw_image(
                                image.clone(),
                                Size::from_wh(1.0, 1.0).ok_or_else(|| size_error("image"))?,
                            );
                            page.pop(1);
                            Ok(())
                        }
                        Resource::Svg(tree) => {
                            let (tree_width, tree_height) =
                                (tree.size().width(), tree.size().height());
                            page.push_transform(
                                scale_x / f64::from(tree_width),
                                0.0,
                                0.0,
                                scale_y / f64::from(tree_height),
                                offset_x,
                                offset_y,
                            )?;
                            // Nested-SVG text is drawn as its outlines, so a
                            // nested SVG embeds no font program.
                            let drawn = super::svg_draw::draw_tree(page.surface, tree);
                            page.pop(1);
                            drawn
                        }
                    })?;
                    page.pop(pushes);
                }
                PdfElementKind::Fallback { source, label } => {
                    if source.pointer.is_empty() {
                        return Err(FormatEncodeError::Encoding(
                            "PDF fallback lost its source location".to_owned(),
                        ));
                    }
                    let mut pushes = page.push_frame_clips(frame_clips)? + 1;
                    page.push_resolved_transform(transform)?;
                    if *opacity != 1.0 {
                        page.push_opacity(*opacity, "PDF fallback opacity")?;
                        pushes += 1;
                    }
                    page.fallback_body(*width, *height, label.as_ref(), self.fallback_colors)?;
                    page.pop(pushes);
                }
            }
        }
        page.pop(1);
        Ok(())
    }
}

/// The surface being drawn, with the push depth it has to return to zero:
/// Krilla asserts a balanced surface when it is dropped, so a failure part way
/// through an element unwinds what that element pushed instead of panicking.
struct Page<'surface, 'fonts, 'a> {
    surface: &'a mut Surface<'surface>,
    fonts: &'fonts PdfFonts,
    tags: Vec<TagGroup>,
    pixels_per_unit: f64,
    depth: usize,
}

impl Page<'_, '_, '_> {
    fn pop(&mut self, count: usize) {
        for _ in 0..count.min(self.depth) {
            self.surface.pop();
            self.depth -= 1;
        }
    }

    fn unwind(&mut self) {
        self.pop(self.depth);
    }

    fn push_clip(&mut self, path: &Path, rule: FillRule) {
        self.surface.push_clip_path(path, &rule);
        self.depth += 1;
    }

    fn push_opacity(&mut self, opacity: f64, subject: &str) -> Result<(), FormatEncodeError> {
        self.surface.push_opacity(normalized(opacity, subject)?);
        self.depth += 1;
        Ok(())
    }

    fn push_transform(
        &mut self,
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        e: f64,
        f: f64,
    ) -> Result<(), FormatEncodeError> {
        let subject = "PDF transformation component";
        self.surface.push_transform(&Transform::from_row(
            finite_f32(a, subject)?,
            finite_f32(b, subject)?,
            finite_f32(c, subject)?,
            finite_f32(d, subject)?,
            finite_f32(e, subject)?,
            finite_f32(f, subject)?,
        ));
        self.depth += 1;
        Ok(())
    }

    /// Places a resolved transform's local origin: rotation about the element
    /// center, then translation, as the SVG `translate() rotate()` pair
    /// composes.
    fn push_resolved_transform(
        &mut self,
        transform: &ResolvedTransform,
    ) -> Result<(), FormatEncodeError> {
        let (sin, cos) = if transform.angle_radians == 0.0 {
            (0.0, 1.0)
        } else {
            sin_cos(transform.angle_radians)
        };
        let offset_x = transform.translation_x + transform.center_x - cos * transform.center_x
            + sin * transform.center_y;
        let offset_y = transform.translation_y + transform.center_y
            - sin * transform.center_x
            - cos * transform.center_y;
        self.push_transform(cos, sin, -sin, cos, offset_x, offset_y)
    }

    /// Installs each frame clip, mapped into page space at full precision
    /// before the element's transform, and returns how many were pushed.
    fn push_frame_clips(&mut self, clips: &[ResolvedClip]) -> Result<usize, FormatEncodeError> {
        for clip in clips {
            let (sin, cos) = sin_cos(clip.transform.angle_radians);
            let offset_x = clip.transform.translation_x + clip.transform.center_x
                - cos * clip.transform.center_x
                + sin * clip.transform.center_y;
            let offset_y = clip.transform.translation_y + clip.transform.center_y
                - sin * clip.transform.center_x
                - cos * clip.transform.center_y;
            let path = exact_path(
                &clip.path,
                |x, y| (cos * x - sin * y + offset_x, sin * x + cos * y + offset_y),
                "PDF frame clip",
            )?;
            self.push_clip(&path, FillRule::NonZero);
        }
        Ok(clips.len())
    }

    /// Draws `draw` marked as an artifact.
    fn artifact(
        &mut self,
        draw: impl FnOnce(&mut Self) -> Result<(), FormatEncodeError>,
    ) -> Result<(), FormatEncodeError> {
        self.surface
            .start_tagged(ContentTag::Artifact(Artifact::with_kind(
                ArtifactType::Other,
            )));
        let result = draw(self);
        self.surface.end_tagged();
        result
    }

    fn fill(
        &mut self,
        path: &Path,
        color: CanonicalColor,
        opacity: f64,
        rule: FillRule,
    ) -> Result<(), FormatEncodeError> {
        self.surface.set_stroke(None);
        self.surface.set_fill(Some(Fill {
            paint: paint(color).into(),
            opacity: normalized(opacity, "PDF fill opacity")?,
            rule,
        }));
        self.surface.draw_path(path);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn stroke(
        &mut self,
        path: &Path,
        color: CanonicalColor,
        opacity: f64,
        width: f64,
        line_cap: LineCap,
        line_join: LineJoin,
        dash: Option<StrokeDash>,
    ) -> Result<(), FormatEncodeError> {
        self.surface.set_fill(None);
        self.surface.set_stroke(Some(Stroke {
            paint: paint(color).into(),
            width: finite_f32(width, "PDF stroke width")?,
            line_cap,
            line_join,
            opacity: normalized(opacity, "PDF stroke opacity")?,
            dash,
            ..Stroke::default()
        }));
        self.surface.draw_path(path);
        self.surface.set_stroke(None);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_drawing(
        &mut self,
        drawing: &Drawing,
        stroke_color: CanonicalColor,
        fill_color: Option<CanonicalColor>,
        stroke_width: f64,
        stroke_style: StrokeStyle,
        source_stroke_width: f64,
        opacity: f64,
    ) -> Result<(), FormatEncodeError> {
        if let (Some(fill), Some(fill_color)) = (&drawing.fill, fill_color) {
            let fill_opacity = opacity * fill_color.alpha();
            match fill.kind {
                FillKind::Solid => self.fill(
                    &rough_path(&fill.path)?,
                    fill_color,
                    fill_opacity,
                    if drawing.shape.canvas_uses_even_odd_fill() {
                        FillRule::EvenOdd
                    } else {
                        FillRule::NonZero
                    },
                )?,
                FillKind::Sketch => self.sketch_stroke(
                    &fill.path,
                    fill_color,
                    fill_opacity,
                    source_stroke_width / 2.0,
                )?,
            }
        }
        let dash = match stroke_style {
            StrokeStyle::Solid => None,
            StrokeStyle::Dashed => Some(dash(8.0, 8.0 + source_stroke_width)?),
            StrokeStyle::Dotted => Some(dash(1.5, 6.0 + source_stroke_width)?),
        };
        if drawing.outline.operations.is_empty() {
            return Ok(());
        }
        self.stroke(
            &rough_path(&drawing.outline)?,
            stroke_color,
            opacity * stroke_color.alpha(),
            stroke_width,
            LineCap::Round,
            LineJoin::Miter,
            dash,
        )
    }

    /// Strokes a sketch fill, starting another path at a subpath boundary
    /// once the running length exceeds the budget. Separate paths composite
    /// like one only when the paint is opaque, so it splits only then. Length
    /// is measured along the control polygon, an upper bound on the curve.
    fn sketch_stroke(
        &mut self,
        path: &SourcePath,
        color: CanonicalColor,
        opacity: f64,
        width: f64,
    ) -> Result<(), FormatEncodeError> {
        let split = opacity == 1.0;
        let budget = SKETCH_STROKE_LENGTH_BUDGET_PX / self.pixels_per_unit;
        let distance =
            |a: Point, b: Point| ((b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y)).sqrt();
        let mut length = 0.0;
        let mut current = None;
        let mut builder = PathBuilder::new();
        let mut empty = true;
        for operation in &path.operations {
            match *operation {
                PathOperation::Move(point) => {
                    if split && length > budget && !empty {
                        let full = std::mem::replace(&mut builder, PathBuilder::new());
                        let finished = finish_path(full, "PDF sketch fill")?;
                        self.stroke(
                            &finished,
                            color,
                            opacity,
                            width,
                            LineCap::Round,
                            LineJoin::Miter,
                            None,
                        )?;
                        length = 0.0;
                    }
                    current = Some(point);
                }
                PathOperation::Line(point) => {
                    length += current.map_or(0.0, |start| distance(start, point));
                    current = Some(point);
                }
                PathOperation::Cubic {
                    control_1,
                    control_2,
                    end,
                } => {
                    length += current.map_or(0.0, |start| distance(start, control_1))
                        + distance(control_1, control_2)
                        + distance(control_2, end);
                    current = Some(end);
                }
            }
            push_rough_operation(&mut builder, operation)?;
            empty = false;
        }
        if !empty {
            let finished = finish_path(builder, "PDF sketch fill")?;
            self.stroke(
                &finished,
                color,
                opacity,
                width,
                LineCap::Round,
                LineJoin::Miter,
                None,
            )?;
        }
        Ok(())
    }

    /// Draws a text element's lines, each a `Span` with its `/ActualText`,
    /// together one `P`.
    fn text(
        &mut self,
        text: &ResolvedText,
        color: CanonicalColor,
        opacity: f64,
    ) -> Result<(), FormatEncodeError> {
        self.set_text_fill(color, opacity)?;
        let mut group = TagGroup::new(Tag::P);
        for line in &text.lines {
            let identifier = self.surface.start_tagged(ContentTag::Span(
                SpanTag::empty().with_actual_text(Some(&line.text)),
            ));
            let result = self.line(text, line);
            self.surface.end_tagged();
            result?;
            group.push(identifier);
        }
        self.surface.set_fill(None);
        self.tags.push(group);
        Ok(())
    }

    fn line(&mut self, text: &ResolvedText, line: &ResolvedLabel) -> Result<(), FormatEncodeError> {
        let fonts = self.fonts;
        if line.runs.is_empty() {
            return draw_glyphs(
                self.surface,
                &official_font(fonts, text.font_index)?.font,
                text.font_size,
                &line.glyphs,
            );
        }
        for run in &line.runs {
            let glyphs = line
                .glyphs
                .get(run.glyph_start..run.glyph_end)
                .ok_or_else(|| {
                    FormatEncodeError::Encoding("PDF text run has invalid glyph bounds".to_owned())
                })?;
            draw_glyphs(
                self.surface,
                &official_font(fonts, run.font_index)?.font,
                text.font_size,
                glyphs,
            )?;
        }
        Ok(())
    }

    /// Draws a Liberation Sans label at 14 as one `Span` in its own `P`.
    fn label(
        &mut self,
        label: &ResolvedLabel,
        color: CanonicalColor,
    ) -> Result<(), FormatEncodeError> {
        let fonts = self.fonts;
        let font = &fonts
            .fallback
            .as_ref()
            .ok_or_else(|| {
                FormatEncodeError::Encoding("PDF label has no embedded fallback font".to_owned())
            })?
            .font;
        self.set_text_fill(color, 1.0)?;
        let identifier = self.surface.start_tagged(ContentTag::Span(
            SpanTag::empty().with_actual_text(Some(&label.text)),
        ));
        let result = draw_glyphs(self.surface, font, 14.0, &label.glyphs);
        self.surface.end_tagged();
        result?;
        self.surface.set_fill(None);
        let mut group = TagGroup::new(Tag::P);
        group.push(identifier);
        self.tags.push(group);
        Ok(())
    }

    fn set_text_fill(
        &mut self,
        color: CanonicalColor,
        opacity: f64,
    ) -> Result<(), FormatEncodeError> {
        self.surface.set_stroke(None);
        self.surface.set_fill(Some(Fill {
            paint: paint(color).into(),
            opacity: normalized(opacity, "PDF text opacity")?,
            rule: FillRule::NonZero,
        }));
        Ok(())
    }

    fn fallback_body(
        &mut self,
        width: f64,
        height: f64,
        label: Option<&ResolvedLabel>,
        colors: FallbackColors,
    ) -> Result<(), FormatEncodeError> {
        let rectangle = rect_path(0.0, 0.0, width, height)?;
        self.push_clip(&rectangle, FillRule::NonZero);
        self.artifact(|page| {
            page.fill(&rectangle, colors.fill, 1.0, FillRule::NonZero)?;
            if width >= 2.0 && height >= 2.0 {
                page.stroke(
                    &rectangle,
                    colors.stroke,
                    1.0,
                    2.0,
                    LineCap::Butt,
                    LineJoin::Miter,
                    Some(dash(6.0, 4.0)?),
                )?;
                let mut builder = PathBuilder::new();
                builder.move_to(0.0, 0.0);
                builder.line_to(
                    finite_f32(width, "PDF fallback cross width")?,
                    finite_f32(height, "PDF fallback cross height")?,
                );
                builder.move_to(finite_f32(width, "PDF fallback reverse cross width")?, 0.0);
                builder.line_to(
                    0.0,
                    finite_f32(height, "PDF fallback reverse cross height")?,
                );
                let cross = finish_path(builder, "PDF fallback cross")?;
                page.stroke(
                    &cross,
                    colors.stroke,
                    1.0,
                    2.0,
                    LineCap::Butt,
                    LineJoin::Miter,
                    None,
                )?;
            }
            Ok(())
        })?;
        if let Some(label) = label {
            self.label(label, colors.label)?;
        }
        self.pop(1);
        Ok(())
    }
}

/// Device-pixel length at 96 DPI that one sketch-fill stroke operation may
/// carry. PDFium's rasterizer silently stops accumulating coverage once a
/// single path exceeds its cell budget, dropping the geometry drawn last: a
/// cross-hatch of about four thousand page-long lines loses its second
/// direction (#84). Splitting at subpath boundaries keeps every operation far
/// below that budget.
const SKETCH_STROKE_LENGTH_BUDGET_PX: f64 = 500_000.0;

fn official_font(fonts: &PdfFonts, index: usize) -> Result<&PdfFont, FormatEncodeError> {
    fonts.official.get(index).ok_or_else(|| {
        FormatEncodeError::Encoding("PDF text references an unknown font resource".to_owned())
    })
}

fn paint(color: CanonicalColor) -> rgb::Color {
    let [red, green, blue] = color.rgb8();
    rgb::Color::new(red, green, blue)
}

fn normalized(value: f64, subject: &str) -> Result<NormalizedF32, FormatEncodeError> {
    NormalizedF32::new(finite_f32(value, subject)?).ok_or_else(|| {
        FormatEncodeError::Encoding(format!("{subject} must lie between zero and one"))
    })
}

fn dash(on: f64, off: f64) -> Result<StrokeDash, FormatEncodeError> {
    Ok(StrokeDash {
        array: vec![
            finite_f32(on, "PDF dash")?,
            finite_f32(off, "PDF dash gap")?,
        ],
        offset: 0.0,
    })
}

fn size_error(subject: &str) -> FormatEncodeError {
    FormatEncodeError::Encoding(format!("PDF {subject} has no drawable size"))
}

fn finish_path(builder: PathBuilder, subject: &str) -> Result<Path, FormatEncodeError> {
    builder
        .finish()
        .ok_or_else(|| FormatEncodeError::Encoding(format!("{subject} has no drawable path")))
}

fn push_rect(
    builder: &mut PathBuilder,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), FormatEncodeError> {
    let subject = "PDF rectangle";
    let (x, y) = (finite_f32(x, subject)?, finite_f32(y, subject)?);
    let (right, bottom) = (
        finite_f32(x as f64 + width, subject)?,
        finite_f32(y as f64 + height, subject)?,
    );
    builder.move_to(x, y);
    builder.line_to(right, y);
    builder.line_to(right, bottom);
    builder.line_to(x, bottom);
    builder.close();
    Ok(())
}

fn rect_path(x: f64, y: f64, width: f64, height: f64) -> Result<Path, FormatEncodeError> {
    let mut builder = PathBuilder::new();
    push_rect(&mut builder, x, y, width, height)?;
    finish_path(builder, "PDF rectangle")
}

/// A Rough path, every coordinate at the reference's two-decimal precision.
fn rough_path(path: &SourcePath) -> Result<Path, FormatEncodeError> {
    let mut builder = PathBuilder::new();
    for operation in &path.operations {
        push_rough_operation(&mut builder, operation)?;
    }
    finish_path(builder, "PDF Rough path")
}

fn push_rough_operation(
    builder: &mut PathBuilder,
    operation: &PathOperation,
) -> Result<(), FormatEncodeError> {
    match operation {
        PathOperation::Move(point) => builder.move_to(rough(point.x)?, rough(point.y)?),
        PathOperation::Line(point) => builder.line_to(rough(point.x)?, rough(point.y)?),
        PathOperation::Cubic {
            control_1,
            control_2,
            end,
        } => builder.cubic_to(
            rough(control_1.x)?,
            rough(control_1.y)?,
            rough(control_2.x)?,
            rough(control_2.y)?,
            rough(end.x)?,
            rough(end.y)?,
        ),
    }
    Ok(())
}

/// The reference's `toFixed(2)` spelling of a Rough coordinate, as a number.
fn rough(value: f64) -> Result<f32, FormatEncodeError> {
    let mut token = String::new();
    push_rough_number(&mut token, value)?;
    let rounded: f64 = token.parse().map_err(|_| {
        FormatEncodeError::Encoding("Rough path number did not round-trip".to_owned())
    })?;
    finite_f32(rounded, "PDF Rough coordinate")
}

/// `path` through `map` at full precision.
fn exact_path(
    path: &SourcePath,
    map: impl Fn(f64, f64) -> (f64, f64),
    subject: &str,
) -> Result<Path, FormatEncodeError> {
    let point = |source: &Point| -> Result<(f32, f32), FormatEncodeError> {
        let (x, y) = map(source.x, source.y);
        Ok((finite_f32(x, subject)?, finite_f32(y, subject)?))
    };
    let mut builder = PathBuilder::new();
    for operation in &path.operations {
        match operation {
            PathOperation::Move(target) => {
                let (x, y) = point(target)?;
                builder.move_to(x, y);
            }
            PathOperation::Line(target) => {
                let (x, y) = point(target)?;
                builder.line_to(x, y);
            }
            PathOperation::Cubic {
                control_1,
                control_2,
                end,
            } => {
                let (x1, y1) = point(control_1)?;
                let (x2, y2) = point(control_2)?;
                let (x, y) = point(end)?;
                builder.cubic_to(x1, y1, x2, y2, x, y);
            }
        }
    }
    finish_path(builder, subject)
}

fn freedraw_path(path: &freedraw::ProjectedPath) -> Result<Path, FormatEncodeError> {
    let subject = "PDF freedraw coordinate";
    let mut builder = PathBuilder::new();
    let mut current = None;
    for operation in &path.operations {
        match *operation {
            freedraw::PathOperation::Move(point) => {
                builder.move_to(finite_f32(point.x, subject)?, finite_f32(point.y, subject)?);
                current = Some(point);
            }
            freedraw::PathOperation::Quadratic { control, end } => {
                let start = current.ok_or_else(|| {
                    FormatEncodeError::Encoding(
                        "PDF freedraw quadratic has no current point".to_owned(),
                    )
                })?;
                let control_1 = Point {
                    x: start.x + (control.x - start.x) * (2.0 / 3.0),
                    y: start.y + (control.y - start.y) * (2.0 / 3.0),
                };
                let control_2 = Point {
                    x: end.x + (control.x - end.x) * (2.0 / 3.0),
                    y: end.y + (control.y - end.y) * (2.0 / 3.0),
                };
                builder.cubic_to(
                    finite_f32(control_1.x, subject)?,
                    finite_f32(control_1.y, subject)?,
                    finite_f32(control_2.x, subject)?,
                    finite_f32(control_2.y, subject)?,
                    finite_f32(end.x, subject)?,
                    finite_f32(end.y, subject)?,
                );
                current = Some(end);
            }
            freedraw::PathOperation::Line(point) => {
                builder.line_to(finite_f32(point.x, subject)?, finite_f32(point.y, subject)?);
                current = Some(point);
            }
            freedraw::PathOperation::Close => builder.close(),
        }
    }
    finish_path(builder, "PDF freedraw path")
}
