//! The vector-first PDF backend: `PdfPlan` lowers the resolved
//! scene into paint-ordered `PdfElement`s and `encode` writes them through
//! Krilla (ADR-0021) — one page, its link annotations, the embedded scene and
//! the artifact manifest as attachments, and the tag tree the page's text
//! forms. This file also owns the link-annotation geometry and the
//! image-viewport projection the children share.
//!
//! `content` draws the page, `fonts` prepares the embedded Modified Versions
//! and draws glyphs, `nested_svg` the raster-closure analysis over sanitized
//! `usvg` trees and the tree each nested-SVG resource is drawn from, and
//! `raster_preflight` the session's admission of those closures.

mod content;
mod dark_nested_svg;
mod fonts;
mod nested_svg;
mod raster;
mod raster_preflight;
mod raster_svg;
mod svg_draw;

use content::PdfImages;
use fonts::{finite_f32, PdfFonts};
use nested_svg::{collect_raster_closures, svg_draw_trees};

use std::sync::{Arc, OnceLock};

use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::configure::{ConfigurationBuilder, PdfVersion};
use krilla::embed::{AssociationKind, EmbeddedFile, MimeType};
use krilla::geom::{Point as KrillaPoint, Quadrilateral, Size};
use krilla::image::{BitsPerComponent, CustomImage, Image, ImageColorspace};
use krilla::page::PageSettings;
use krilla::tagging::TagTree;
use krilla::{Document, SerializeSettings};

use crate::normalization::NormalizedElementSource;
use crate::portable_math::{cube, sin_cos};
use crate::{color::CanonicalColor, LogicalBounds, NestedPath, RenderRequest};

use super::geometry::{clip_convex_polygon, Path, PathOperation, Point};
use super::metadata::ArtifactMetadata;
use super::notices::MandatoryNotices;
use super::plan::PlannedElement;
use super::resolve::{
    DarkModeImageFilter, FallbackColors, FallbackSource, ResolvedClip, ResolvedElement,
    ResolvedFrameName, ResolvedFrameOutline, ResolvedImageAsset, ResolvedImageProjection,
    ResolvedImageSourceRect, ResolvedLabelMask, ResolvedScene, ResolvedTransform, StrokeStyle,
};
use super::rough::{Drawing, Fill};
use super::svg_filter_work::FilterWork;
use super::text::{ResolvedLabel, ResolvedText};
use super::{freedraw, FormatEncodeError, SvgTreeCache};

pub(super) const POINTS_PER_LOGICAL_PIXEL: f64 = 0.75;

// Project-owned content is drawn directly; krilla-svg draws only sanitized
// nested-SVG trees.
#[derive(Debug)]
pub(super) struct PdfPlan {
    logical_width: f64,
    logical_height: f64,
    scale: f64,
    page_width: f64,
    page_height: f64,
    background: Option<CanonicalColor>,
    fallback_colors: FallbackColors,
    mandatory_notices: Option<MandatoryNotices>,
    metadata: Option<ArtifactMetadata>,
    fonts: PdfFonts,
    svg_trees: SvgTreeCache,
    elements: Vec<PdfElement>,
    raster_closures: Vec<PdfRasterClosure>,
    svg_resources: Vec<PdfSvgResource>,
    /// The nested-SVG trees `encode` draws, lowered on first use: they follow
    /// from the closures and resources alone, which no later step changes.
    svg_draw_trees: OnceLock<Vec<Arc<usvg::Tree>>>,
}

/// One raster image XObject: an asset embedded once however many elements
/// reference it, with the dark-mode filter those elements share (the filter
/// follows from the mode and the asset kind, so they agree).
#[derive(Debug)]
struct PdfImageResource {
    asset: Arc<ResolvedImageAsset>,
    dark_mode_filter: Option<DarkModeImageFilter>,
}

#[derive(Debug)]
pub(super) struct PdfRasterClosure {
    pub(super) source_index: usize,
    raster: raster::RasterPlacement,
    pub(super) element_id: String,
    pub(super) json_pointer: String,
    pub(super) source_order: u64,
    pub(super) source_position: u64,
    pub(super) logical_bounds: LogicalBounds,
    pub(super) pixel_width: u64,
    pub(super) pixel_height: u64,
    pub(super) causes: Vec<NestedPath>,
    pub(super) unisolatable: bool,
}

impl PdfRasterClosure {
    fn shares_input_grid(&self) -> bool {
        self.raster.input_x == self.raster.output_x
            && self.raster.input_y == self.raster.output_y
            && self.raster.input_width == self.pixel_width
            && self.raster.input_height == self.pixel_height
    }
    fn surface_dimensions(&self) -> impl Iterator<Item = (u64, u64)> {
        std::iter::once((self.raster.input_width, self.raster.input_height))
            .chain((!self.shares_input_grid()).then_some((self.pixel_width, self.pixel_height)))
    }
}

#[derive(Debug)]
struct PdfSvgResource {
    asset: Arc<ResolvedImageAsset>,
    source_index: usize,
    prepared: Option<raster::PreparedSvg>,
}

/// One PDF plan element: the shared header in logical units, projected to
/// points at write time, and the kind.
type PdfElement = PlannedElement<ResolvedTransform, ResolvedClip, PdfElementKind>;

/// What differs per element kind in the PDF plan; everything else an element
/// carries is the [`PlannedElement`] header.
#[derive(Debug)]
enum PdfElementKind {
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
        path: freedraw::ProjectedPath,
        background_fill: Option<Fill>,
        color: CanonicalColor,
        background_color: Option<CanonicalColor>,
        source_stroke_width: f64,
    },
    Frame {
        source: NormalizedElementSource,
        name: Option<ResolvedFrameName>,
        outline: Option<ResolvedFrameOutline>,
    },
    Text {
        source: NormalizedElementSource,
        text: ResolvedText,
        color: CanonicalColor,
    },
    Image {
        source_order: u64,
        source: NormalizedElementSource,
        asset: Arc<ResolvedImageAsset>,
        /// The dark-mode filter official SVG applies to this image, which PDF
        /// lowers into a raster asset's pixels.
        dark_mode_filter: Option<DarkModeImageFilter>,
        /// The rounded-corner clip of an image with a roundness record, in
        /// element space; without one the image clips to its rectangle.
        corner_clip: Option<Path>,
        /// The crop in the asset's pixel grid, which bounds the effect regions
        /// a nested SVG rasterizes.
        source_rect: ResolvedImageSourceRect,
        /// The reference export's rounded viewport and content placement.
        projection: ResolvedImageProjection,
        scale: [i8; 2],
    },
    Fallback {
        source: FallbackSource,
        label: Option<ResolvedLabel>,
    },
}

impl PdfPlan {
    pub(super) fn image_resource_pixel_counts(&self) -> Result<Vec<u64>, FormatEncodeError> {
        let mut counts = self
            .image_resources()?
            .iter()
            .map(|resource| {
                u64::from(resource.asset.width)
                    .checked_mul(u64::from(resource.asset.height))
                    .ok_or_else(pdf_size_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        counts.extend(
            self.raster_closures
                .iter()
                .map(|closure| closure.pixel_width.saturating_mul(closure.pixel_height)),
        );
        Ok(counts)
    }

    pub(super) fn new(
        scene: ResolvedScene,
        request: &RenderRequest,
        svg_trees: &SvgTreeCache,
    ) -> Result<Self, FormatEncodeError> {
        let scale = scene.raster_or_pdf_scale(request.sizing());
        let (page_width, page_height) = page_dimensions(&scene, request)?;
        let background = scene.include_background.then_some(scene.background_color);
        let mut elements = Vec::new();
        elements
            .try_reserve_exact(scene.elements.len())
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_plan",
                subject: "pdf_elements",
            })?;
        for element in scene.elements {
            let source_order = element.source_order();
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
                    elements.push(PdfElement {
                        frame_clips,
                        transform: outline.transform,
                        opacity: if outline.generated_label.is_some() {
                            outline.element_opacity
                        } else {
                            outline.opacity
                        },
                        link: outline.link,
                        width: outline.width,
                        height: outline.height,
                        kind: PdfElementKind::RoughOutline {
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
                    elements.push(PdfElement {
                        frame_clips: freedraw.frame_clips,
                        transform: freedraw.transform,
                        opacity: freedraw.opacity,
                        link: freedraw.link,
                        width: freedraw.width,
                        height: freedraw.height,
                        kind: PdfElementKind::Freedraw {
                            source: freedraw.source,
                            path: freedraw::project(freedraw.path)?,
                            background_fill: freedraw.background_fill,
                            color: freedraw.color,
                            background_color: freedraw.background_color,
                            source_stroke_width: freedraw.source_stroke_width,
                        },
                    });
                }
                ResolvedElement::Frame(frame) => {
                    elements.push(PdfElement {
                        frame_clips: frame.frame_clips,
                        transform: frame.transform,
                        opacity: frame.opacity,
                        link: frame.link,
                        width: frame.width,
                        height: frame.height,
                        kind: PdfElementKind::Frame {
                            source: frame.source,
                            name: frame.name,
                            outline: frame.outline,
                        },
                    });
                }
                ResolvedElement::Text(text) => {
                    elements.push(PdfElement {
                        frame_clips: text.frame_clips,
                        transform: text.transform,
                        opacity: text.opacity,
                        link: text.link,
                        width: text.width,
                        height: text.height,
                        kind: PdfElementKind::Text {
                            source: text.source,
                            text: text.text,
                            color: text.color,
                        },
                    });
                }
                ResolvedElement::Image(image) => {
                    elements.push(PdfElement {
                        frame_clips: image.frame_clips,
                        transform: image.transform,
                        opacity: image.opacity,
                        link: image.link,
                        width: image.width,
                        height: image.height,
                        kind: PdfElementKind::Image {
                            source_order,
                            source: image.source,
                            asset: image.asset,
                            dark_mode_filter: image.dark_mode_filter,
                            corner_clip: image.corner_clip.map(|clip| clip.path),
                            source_rect: image.source_rect,
                            projection: image.projection,
                            scale: image.scale,
                        },
                    });
                }
                ResolvedElement::ImageFallback(_) => {}
                ResolvedElement::Fallback(fallback) => {
                    elements.push(PdfElement {
                        frame_clips: fallback.frame_clips,
                        transform: fallback.transform,
                        opacity: fallback.opacity,
                        link: fallback.link,
                        width: fallback.width,
                        height: fallback.height,
                        kind: PdfElementKind::Fallback {
                            source: fallback.source,
                            label: fallback.label,
                        },
                    });
                }
            }
        }
        // Every embedded program is a Modified Version (ADR-0021), so the
        // artifact's notices name each program beside its family's.
        let fonts = PdfFonts::prepare(scene.fallback_font_sfnt.as_deref(), &scene.official_fonts)?;
        let mandatory_notices = scene
            .mandatory_notices
            .map(|_| fonts.notices(&scene.official_fonts))
            .transpose()?;
        let mut plan = Self {
            logical_width: scene.logical_width,
            logical_height: scene.logical_height,
            scale,
            svg_trees: if request.color_mode() == crate::ColorMode::Dark {
                // The reference's root filter inverts nested-SVG content with
                // the scene; PDF lowers it into that content (#154).
                svg_trees
                    .iter()
                    .map(|(identity, tree)| {
                        dark_nested_svg::lower(tree).map(|lowered| (*identity, Arc::new(lowered)))
                    })
                    .collect::<Result<_, _>>()?
            } else {
                svg_trees.clone()
            },
            page_width,
            page_height,
            background,
            fallback_colors: scene.fallback_colors,
            mandatory_notices,
            metadata: None,
            fonts,
            elements,
            raster_closures: Vec::new(),
            svg_resources: Vec::new(),
            svg_draw_trees: OnceLock::new(),
        };
        plan.svg_resources = plan.build_svg_resources()?;
        plan.raster_closures = collect_raster_closures(
            &plan.elements,
            &plan.svg_resources,
            scale,
            plan.logical_width,
            plan.logical_height,
        )?;
        Ok(plan)
    }

    pub(super) fn raster_closures(&self) -> &[PdfRasterClosure] {
        &self.raster_closures
    }

    /// What resvg will spend on the filter effects of `closure` when it is
    /// rasterized, in its input surface's device pixels. The walk stops past
    /// `budget` work units.
    pub(super) fn raster_closure_filter_work(
        &self,
        closure: &PdfRasterClosure,
        budget: u64,
    ) -> Result<FilterWork, FormatEncodeError> {
        self.svg_resources
            .iter()
            .find(|resource| resource.source_index == closure.source_index)
            .and_then(|resource| resource.prepared.as_ref())
            .map_or(Ok(FilterWork::default()), |prepared| {
                prepared.filter_work(closure, budget)
            })
    }

    pub(super) fn take_mandatory_notices(&mut self) -> Option<MandatoryNotices> {
        self.mandatory_notices.take()
    }

    pub(super) fn set_metadata(&mut self, metadata: ArtifactMetadata) {
        self.metadata = Some(metadata);
        self.mandatory_notices = None;
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>, FormatEncodeError> {
        let page_size = Size::from_wh(
            finite_f32(self.page_width, "PDF page width")?,
            finite_f32(self.page_height, "PDF page height")?,
        )
        .ok_or_else(|| {
            FormatEncodeError::Encoding(
                "PDF page rounds to zero points at the requested size".to_owned(),
            )
        })?;
        let raster = self
            .image_resources()?
            .iter()
            .map(raster_image)
            .collect::<Result<Vec<_>, _>>()?;
        // Lowering rasterizes every closure, which the raster preflight
        // charged once; the measured candidates and the emission share it.
        let svg_trees = if let Some(trees) = self.svg_draw_trees.get() {
            trees.clone()
        } else {
            let trees =
                svg_draw_trees(&self.svg_resources, &self.svg_trees, &self.raster_closures)?;
            self.svg_draw_trees.get_or_init(|| trees).clone()
        };
        let annotations = self.link_annotations()?;

        let mut document = Document::new_with(serialize_settings()?);
        // The crop box restates the media box, as the page has always said.
        let page_box =
            krilla::geom::Rect::from_xywh(0.0, 0.0, page_size.width(), page_size.height());
        let mut page =
            document.start_page_with(PageSettings::new(page_size).with_crop_box(page_box));
        let mut surface = page.surface();
        let tags = self.draw_page(
            &mut surface,
            &self.fonts,
            &PdfImages {
                raster,
                svg: svg_trees,
            },
        );
        surface.finish();
        for annotation in annotations {
            page.add_annotation(annotation);
        }
        page.finish();
        let mut tree = TagTree::new();
        for group in tags? {
            tree.push(group);
        }
        document.set_tag_tree(tree);

        // The artifact manifest and the canonical scene are attachments; the
        // manifest carries the mandatory notices (spec 12.2, ADR-0021).
        if let Some(manifest) = self.metadata.as_ref().and_then(ArtifactMetadata::manifest) {
            super::metadata::operation(super::metadata::Stage::ManifestEmbedding, || {
                embed(
                    &mut document,
                    MANIFEST_ATTACHMENT,
                    "application/json",
                    "excalidraw-render artifact manifest",
                    AssociationKind::Supplement,
                    manifest.as_bytes(),
                )
            })?;
        }
        if let Some(scene) = self
            .metadata
            .as_ref()
            .and_then(ArtifactMetadata::canonical_scene)
        {
            super::metadata::operation(super::metadata::Stage::SceneEmbedding, || {
                embed(
                    &mut document,
                    "scene.excalidraw",
                    "application/vnd.excalidraw+json",
                    "Canonical Excalidraw scene",
                    AssociationKind::Data,
                    scene,
                )
            })?;
        }
        document
            .finish()
            .map_err(|error| FormatEncodeError::Encoding(format!("failed to write PDF: {error:?}")))
    }

    pub(super) fn source_count(&self) -> usize {
        self.elements.len()
    }

    /// The cluster text the element at `index` repeats per glyph across every
    /// label it draws (spec 13.2), charged before Krilla compares it.
    pub(super) fn repeated_cluster_text(&self, index: usize) -> u64 {
        let text = |text: &ResolvedText| {
            text.lines
                .iter()
                .map(ResolvedLabel::repeated_cluster_text)
                .fold(0, u64::saturating_add)
        };
        self.elements
            .get(index)
            .map_or(0, |element| match &element.kind {
                PdfElementKind::Text { text: label, .. } => text(label),
                PdfElementKind::RoughOutline {
                    generated_label, ..
                } => generated_label.as_ref().map_or(0, text),
                PdfElementKind::Frame { name, .. } => name
                    .as_ref()
                    .map_or(0, |name| name.label.repeated_cluster_text()),
                PdfElementKind::Fallback { label, .. } => label
                    .as_ref()
                    .map_or(0, ResolvedLabel::repeated_cluster_text),
                PdfElementKind::Freedraw { .. } | PdfElementKind::Image { .. } => 0,
            })
    }

    pub(super) fn source(&self, index: usize) -> Option<(Option<&str>, &str)> {
        self.elements.get(index).map(|element| match &element.kind {
            PdfElementKind::RoughOutline { source, .. }
            | PdfElementKind::Freedraw { source, .. }
            | PdfElementKind::Frame { source, .. }
            | PdfElementKind::Text { source, .. }
            | PdfElementKind::Image { source, .. } => (Some(source.id()), source.pointer()),
            PdfElementKind::Fallback { source, .. } => {
                (source.id.as_deref(), source.pointer.as_str())
            }
        })
    }

    /// One borderless URI annotation per visible linked element, topmost
    /// first, over the element's clipped hit area in Krilla's top-down page
    /// space.
    fn link_annotations(&self) -> Result<Vec<Annotation>, FormatEncodeError> {
        let point_scale = self.scale * POINTS_PER_LOGICAL_PIXEL;
        let mut annotations = Vec::new();
        for element in self.elements.iter().rev() {
            let Some(link) = element.link.as_deref() else {
                continue;
            };
            let points = clipped_link_polygon(
                element.width,
                element.height,
                &element.transform,
                &element.frame_clips,
                point_scale,
                self.page_width,
                self.page_height,
            )?;
            if points.len() < 3 {
                continue;
            }
            let point = |(x, y): (f64, f64)| -> Result<KrillaPoint, FormatEncodeError> {
                Ok(KrillaPoint::from_xy(
                    finite_f32(x, "PDF link x")?,
                    finite_f32(self.page_height - y, "PDF link y")?,
                ))
            };
            let mut quadrilaterals = Vec::new();
            if points.len() == 4 {
                quadrilaterals.push(Quadrilateral([
                    point(points[0])?,
                    point(points[1])?,
                    point(points[3])?,
                    point(points[2])?,
                ]));
            } else {
                for index in 1..points.len() - 1 {
                    quadrilaterals.push(Quadrilateral([
                        point(points[0])?,
                        point(points[index])?,
                        point(points[index + 1])?,
                        point(points[index + 1])?,
                    ]));
                }
            }
            annotations
                .try_reserve(1)
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "format_workspace",
                    subject: "pdf_link_annotations",
                })?;
            annotations.push(Annotation::new_link(
                LinkAnnotation::new_with_quad_points(
                    quadrilaterals,
                    Target::Action(Action::Link(LinkAction::new(link_uri(link)))),
                ),
                None,
            ));
        }
        Ok(annotations)
    }

    fn image_resources(&self) -> Result<Vec<PdfImageResource>, FormatEncodeError> {
        let mut resources = Vec::new();
        resources
            .try_reserve_exact(self.elements.len())
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_workspace",
                subject: "pdf_image_resources",
            })?;
        for element in &self.elements {
            let PdfElementKind::Image {
                asset,
                dark_mode_filter,
                ..
            } = &element.kind
            else {
                continue;
            };
            if asset.sanitized_svg.is_some() {
                continue;
            }
            if resources
                .iter()
                .any(|candidate: &PdfImageResource| candidate.asset.identity == asset.identity)
            {
                continue;
            }
            resources.push(PdfImageResource {
                asset: Arc::clone(asset),
                dark_mode_filter: *dark_mode_filter,
            });
        }
        resources.sort_unstable_by_key(|resource| resource.asset.identity);
        Ok(resources)
    }

    fn build_svg_resources(&self) -> Result<Vec<PdfSvgResource>, FormatEncodeError> {
        let mut resources = Vec::new();
        resources
            .try_reserve_exact(self.elements.len())
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_workspace",
                subject: "pdf_svg_resources",
            })?;
        for element in &self.elements {
            let PdfElementKind::Image { asset, source, .. } = &element.kind else {
                continue;
            };
            if asset.sanitized_svg.is_none() {
                continue;
            }
            let tree = self
                .svg_trees
                .get(&asset.identity)
                .ok_or_else(pdf_size_error)?;
            let has_effects = !asset.svg_effects.is_empty() || !tree.filters().is_empty();
            if resources.iter().any(|candidate: &PdfSvgResource| {
                if has_effects {
                    candidate.source_index == source.source_index()
                } else {
                    candidate.asset.identity == asset.identity
                }
            }) {
                continue;
            }
            let prepared = if has_effects {
                Some(raster::PreparedSvg::new(tree)?)
            } else {
                None
            };
            resources.push(PdfSvgResource {
                asset: Arc::clone(asset),
                source_index: source.source_index(),
                prepared,
            });
        }
        resources.sort_by_key(|resource| resource.asset.identity);
        Ok(resources)
    }
}

pub(super) fn page_dimensions(
    scene: &ResolvedScene,
    request: &RenderRequest,
) -> Result<(f64, f64), FormatEncodeError> {
    let scale = scene.raster_or_pdf_scale(request.sizing());
    Ok((
        point_dimension(scene.logical_width * scale)?,
        point_dimension(scene.logical_height * scale)?,
    ))
}

fn transformed_pdf_points(
    width: f64,
    height: f64,
    transform: &ResolvedTransform,
    point_scale: f64,
    page_height: f64,
) -> [(f64, f64); 4] {
    let (sin, cos) = sin_cos(transform.angle_radians);
    let offset_x = transform.translation_x + transform.center_x - cos * transform.center_x
        + sin * transform.center_y;
    let offset_y = transform.translation_y + transform.center_y
        - sin * transform.center_x
        - cos * transform.center_y;
    let map = |x: f64, y: f64| {
        let logical_x = cos * x - sin * y + offset_x;
        let logical_y = sin * x + cos * y + offset_y;
        (
            logical_x * point_scale,
            page_height - logical_y * point_scale,
        )
    };
    [
        map(0.0, 0.0),
        map(width, 0.0),
        map(0.0, height),
        map(width, height),
    ]
}

fn clipped_link_polygon(
    width: f64,
    height: f64,
    transform: &ResolvedTransform,
    frame_clips: &[ResolvedClip],
    point_scale: f64,
    page_width: f64,
    page_height: f64,
) -> Result<Vec<(f64, f64)>, FormatEncodeError> {
    let element = transformed_pdf_points(width, height, transform, point_scale, page_height);
    let mut polygon = Vec::new();
    polygon
        .try_reserve_exact(4)
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_workspace",
            subject: "pdf_link_polygon",
        })?;
    polygon.extend([element[0], element[1], element[3], element[2]]);
    polygon = clip_convex_polygon(
        &polygon,
        &[
            (0.0, 0.0),
            (0.0, page_height),
            (page_width, page_height),
            (page_width, 0.0),
        ],
    )?;
    for frame_clip in frame_clips {
        let frame = transformed_clip_path(frame_clip, point_scale, page_height)?;
        polygon = clip_convex_polygon(&polygon, &frame)?;
    }
    Ok(polygon)
}

fn transformed_clip_path(
    clip: &ResolvedClip,
    point_scale: f64,
    page_height: f64,
) -> Result<Vec<(f64, f64)>, FormatEncodeError> {
    const CURVE_STEPS: usize = 8;
    let mut points = Vec::new();
    points
        .try_reserve_exact(clip.path.operations.len().saturating_mul(CURVE_STEPS))
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_workspace",
            subject: "pdf_link_frame_clip",
        })?;
    let mut current = Point { x: 0.0, y: 0.0 };
    for operation in &clip.path.operations {
        match operation {
            PathOperation::Move(point) => {
                current = *point;
                points.push(transformed_clip_point(
                    current,
                    &clip.transform,
                    point_scale,
                    page_height,
                ));
            }
            PathOperation::Line(point) => {
                current = *point;
                points.push(transformed_clip_point(
                    current,
                    &clip.transform,
                    point_scale,
                    page_height,
                ));
            }
            PathOperation::Cubic {
                control_1,
                control_2,
                end,
            } => {
                let start = current;
                for step in 1..=CURVE_STEPS {
                    let t = step as f64 / CURVE_STEPS as f64;
                    let inverse = 1.0 - t;
                    let point = Point {
                        x: cube(inverse) * start.x
                            + 3.0 * inverse * inverse * t * control_1.x
                            + 3.0 * inverse * t * t * control_2.x
                            + cube(t) * end.x,
                        y: cube(inverse) * start.y
                            + 3.0 * inverse * inverse * t * control_1.y
                            + 3.0 * inverse * t * t * control_2.y
                            + cube(t) * end.y,
                    };
                    points.push(transformed_clip_point(
                        point,
                        &clip.transform,
                        point_scale,
                        page_height,
                    ));
                }
                current = *end;
            }
        }
    }
    Ok(points)
}

fn transformed_clip_point(
    point: Point,
    transform: &ResolvedTransform,
    point_scale: f64,
    page_height: f64,
) -> (f64, f64) {
    let (sin, cos) = sin_cos(transform.angle_radians);
    let delta_x = point.x - transform.center_x;
    let delta_y = point.y - transform.center_y;
    let x = transform.translation_x + transform.center_x + delta_x * cos - delta_y * sin;
    let y = transform.translation_y + transform.center_y + delta_x * sin + delta_y * cos;
    (x * point_scale, page_height - y * point_scale)
}

/// The name the artifact manifest is attached under (spec 12.2).
const MANIFEST_ATTACHMENT: &str = "artifact-manifest.json";

/// Krilla's settings for every artifact: PDF 1.7 with no conformance claim,
/// every colour in Krilla's ICC-based sRGB rather than a device space, Flate
/// content streams, no XMP packet (the manifest is an attachment), and tagging
/// on for the page's text structure (ADR-0021).
fn serialize_settings() -> Result<SerializeSettings, FormatEncodeError> {
    let configuration = ConfigurationBuilder::new()
        .with_version(PdfVersion::Pdf17)
        .finish()
        .map_err(|error| {
            FormatEncodeError::Encoding(format!("invalid PDF configuration: {error:?}"))
        })?;
    Ok(SerializeSettings {
        compress_content_streams: true,
        no_device_cs: true,
        ascii_compatible: false,
        xmp_metadata: false,
        enable_tagging: true,
        configuration,
        ..SerializeSettings::default()
    })
}

fn embed(
    document: &mut Document,
    name: &str,
    media_type: &str,
    description: &str,
    association: AssociationKind,
    bytes: &[u8],
) -> Result<(), FormatEncodeError> {
    let mut data = Vec::new();
    data.try_reserve_exact(bytes.len())
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "metadata",
            subject: "pdf_attachment",
        })?;
    data.extend_from_slice(bytes);
    document
        .embed_file(EmbeddedFile {
            path: name.to_owned(),
            mime_type: MimeType::new(media_type),
            description: Some(description.to_owned()),
            association_kind: association,
            data: data.into(),
            modification_date: None,
            compress: Some(true),
            location: None,
        })
        .ok_or_else(|| FormatEncodeError::Encoding(format!("PDF attachment {name} is not unique")))
}

/// One image resource's two planes: the RGB samples, lowered through the
/// dark-mode filter when the mode asks for it, and the alpha soft mask.
#[derive(Clone, Hash)]
struct ImagePlanes {
    rgb: Arc<[u8]>,
    alpha: Arc<[u8]>,
    width: u32,
    height: u32,
}

impl CustomImage for ImagePlanes {
    fn color_channel(&self) -> &[u8] {
        &self.rgb
    }
    fn alpha_channel(&self) -> Option<&[u8]> {
        Some(&self.alpha)
    }
    fn bits_per_component(&self) -> BitsPerComponent {
        BitsPerComponent::Eight
    }
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn icc_profile(&self) -> Option<&[u8]> {
        None
    }
    fn color_space(&self) -> ImageColorspace {
        ImageColorspace::Rgb
    }
}

fn raster_image(resource: &PdfImageResource) -> Result<Image, FormatEncodeError> {
    let asset = &resource.asset;
    let pixel_count = usize::try_from(asset.width)
        .ok()
        .and_then(|width| width.checked_mul(usize::try_from(asset.height).ok()?))
        .ok_or_else(pdf_size_error)?;
    let mut rgb = Vec::new();
    let mut alpha = Vec::new();
    let rgb_bytes = pixel_count.checked_mul(3).ok_or_else(pdf_size_error)?;
    rgb.try_reserve_exact(rgb_bytes)
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_resource",
            subject: "pdf_image_rgb",
        })?;
    alpha
        .try_reserve_exact(pixel_count)
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_resource",
            subject: "pdf_image_alpha",
        })?;
    let rgba = asset.rgba.as_deref().ok_or_else(|| {
        FormatEncodeError::Encoding("PDF image has no normalized pixel resource".to_owned())
    })?;
    if rgba.len() != pixel_count.saturating_mul(4) {
        return Err(FormatEncodeError::Encoding(
            "PDF image pixels disagree with its dimensions".to_owned(),
        ));
    }
    for pixel in rgba.as_chunks::<4>().0 {
        let [red, green, blue, pixel_alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        // Official SVG is the visual authority for PDF images (spec
        // section 8.6): in dark mode a raster pixel passes through the
        // reference's image filter and root filter. An invisible pixel keeps
        // its canonical zero RGB.
        let lowered = match resource.dark_mode_filter {
            Some(DarkModeImageFilter::RasterImage) if pixel_alpha != 0 => {
                crate::color::dark_mode_raster_image_pixel([red, green, blue])
            }
            _ => [red, green, blue],
        };
        rgb.extend_from_slice(&lowered);
        alpha.push(pixel_alpha);
    }
    Image::from_custom(
        ImagePlanes {
            rgb: rgb.into(),
            alpha: alpha.into(),
            width: asset.width,
            height: asset.height,
        },
        true,
    )
    .map_err(|error| FormatEncodeError::Encoding(format!("PDF image is not embeddable: {error}")))
}

/// The URI a link annotation opens: a link without a scheme resolves against
/// the reference's placeholder origin, as the reference export does.
fn link_uri(link: &str) -> String {
    let path_end = link.find(['/', '?', '#']).unwrap_or(link.len());
    let has_scheme = link
        .find(':')
        .is_some_and(|scheme_end| scheme_end < path_end);
    match (has_scheme, link.starts_with('/')) {
        (true, _) => link.to_owned(),
        (false, true) => format!("http://reference.invalid{link}"),
        (false, false) => format!("http://reference.invalid/{link}"),
    }
}

fn pdf_size_error() -> FormatEncodeError {
    FormatEncodeError::Encoding("PDF plan exceeds serializer capacity".to_owned())
}

/// The rectangle of the element's space the reference leaves visible, as
/// `(x, y, width, height)` after the flip: the `<use>` viewport as the flip
/// mirrors it about the element's unrounded center, intersected with the crop
/// `<mask>`, which sits on the unflipped group, when the image is cropped.
fn visible_rect(
    projection: &ResolvedImageProjection,
    width: f64,
    height: f64,
    scale: [i8; 2],
) -> (f64, f64, f64, f64) {
    let (x, y, viewport_width, viewport_height) = projection.viewport_rect();
    let mut left = if scale[0] < 0 {
        width - x - viewport_width
    } else {
        x
    };
    let mut top = if scale[1] < 0 {
        height - y - viewport_height
    } else {
        y
    };
    let mut right = left + viewport_width;
    let mut bottom = top + viewport_height;
    if projection.crop.is_some() {
        left = left.max(0.0);
        top = top.max(0.0);
        right = right.min(projection.viewport_width);
        bottom = bottom.min(projection.viewport_height);
    }
    (left, top, right - left, bottom - top)
}

fn point_dimension(logical: f64) -> Result<f64, FormatEncodeError> {
    let points = logical * POINTS_PER_LOGICAL_PIXEL;
    if !points.is_finite() || points <= 0.0 {
        return Err(FormatEncodeError::Encoding(
            "PDF page dimensions must be finite positive point values".to_owned(),
        ));
    }
    Ok(points)
}
