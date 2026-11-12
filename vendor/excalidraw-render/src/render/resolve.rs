//! The format-neutral semantic graph. `ResolvedScene::elements`
//! orchestrates one scene's resolution — bound-text placement and masks, the
//! fallback font and frame names, the scene bounds, the semantic frames, one
//! resolved element per surviving source element in source order, the painted
//! placeholders, the paint-order sort and the font and notice assembly — and
//! this file owns every type in the graph, the `ResolveContext` the per-kind
//! resolutions read, the asset location helpers and the element-local resource
//! crossings.
//!
//! `bounds` is the layout pass; `frame`, `text`, `image`, `outline` and
//! `fallback` each resolve one element family. Resolution is not charged, but
//! `account_fallback_semantics` charges the graph it produces in paint order
//! (ADR-0002), so the element loop's order and every early return are the
//! contract.

pub(super) mod admission;
mod bounds;
mod fallback;
mod frame;
mod image;
pub(super) mod outline;
pub(super) mod text;

use bounds::scene_bounds;
use fallback::{fallback_frame_clip_sources, resolve_fallback};
use frame::{resolve_frame, resolve_frame_names, semantic_frame_sources};
use image::resolve_image;
use outline::resolve_geometry;
pub(super) use text::live_text_occurrence;
use text::{resolve_fonts, resolve_text};

use crate::color::{self, CanonicalColor, ColorLowering};
use crate::diagnostic_registry::{DiagnosticKind, LocalLimit, LocalRecovery};
use crate::normalization::{NormalizedElement, NormalizedElementKind, NormalizedElementSource};
use crate::outcome::PendingDiagnostic;
use crate::relationships::ResolvedRelationshipGraph;
use crate::resource_accounting::ResourceDelta;
use crate::scene_surface::SurfaceFallback;
use crate::{
    DiagnosticEffect, ErrorCode, FiniteValue, OutputFormat, RenderRequest, ResourceValue,
    SceneDocument,
};

use super::bound_text::{
    resolve_bound_text_masks, resolve_bound_text_placements, BoundTextMaskSource, SourcePositions,
};
use super::font_demand::{prune_unused_official_fonts, ResolvedFont};
use super::frame_target::{
    resolve_frame_clips, resolve_frame_opacity, EffectiveFrameRendering, FrameViewport,
};
use super::geometry::{
    ecmascript_round, image_corner_radius, magic_frame_canvas_outline_stroke_color,
    resolved_frame_outline, resolved_frame_rect, rounded_rect_path, GeneratedGeometry, Path,
    FRAME_OUTLINE_STROKE_COLOR, FRAME_RADIUS,
};
use super::text::{
    mandatory_notices_json, CjkSubsetTrigger, FallbackFont, LabelError, OfficialFont,
    ResolvedLabel, ResolvedText,
};
use super::{asset, freedraw, notices, rough, try_owned};

#[derive(Debug)]
pub(super) struct ResolvedScene {
    pub(super) coordinate_omissions: Vec<admission::CoordinateOmission>,
    pub(super) logical_width: f64,
    pub(super) logical_height: f64,
    pub(super) include_background: bool,
    pub(super) background_color: CanonicalColor,
    pub(super) fallback_colors: FallbackColors,
    pub(super) elements: Vec<ResolvedElement>,
    pub(super) fallback_font_woff2: Option<Vec<u8>>,
    pub(super) fallback_font_sfnt: Option<std::sync::Arc<[u8]>>,
    pub(super) official_fonts: Vec<ResolvedFont>,
    pub(super) mandatory_notices: Option<notices::MandatoryNotices>,
    /// The CJK subset trigger's outcome, shared by glyph selection and the SVG
    /// font demand so they select Xiaolai together or not at all.
    pub(super) cjk_subset_trigger: CjkSubsetTrigger,
}

pub(super) fn semantic_identity_hex(
    identity: &[u8; 32],
) -> Result<String, std::collections::TryReserveError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::new();
    encoded.try_reserve_exact(64)?;
    for byte in identity {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

pub(super) fn relationship_asset_authorities(
    relationships: &crate::relationships::ResolvedRelationshipGraph,
) -> Result<Vec<(usize, asset::Authority)>, std::collections::TryReserveError> {
    let mut authorities = Vec::new();
    authorities.try_reserve_exact(relationships.edges.len())?;
    for edge in &relationships.edges {
        if edge.kind != crate::relationships::RelationshipKind::Asset {
            continue;
        }
        let Some(node) = relationships.nodes.get(edge.source) else {
            continue;
        };
        let authority = match edge.target {
            crate::relationships::RelationshipTarget::Asset {
                authority: crate::relationships::AssetAuthority::SceneFile,
                ..
            } => asset::Authority::Scene,
            crate::relationships::RelationshipTarget::Asset {
                authority: crate::relationships::AssetAuthority::Supplemental,
                ..
            } => asset::Authority::Supplemental,
            crate::relationships::RelationshipTarget::Asset {
                authority: crate::relationships::AssetAuthority::Missing,
                ..
            } => asset::Authority::Missing,
            _ => continue,
        };
        authorities.push((node.source_index, authority));
    }
    authorities.sort_unstable_by_key(|(source_index, _)| *source_index);
    authorities.dedup_by_key(|(source_index, _)| *source_index);
    Ok(authorities)
}

pub(super) fn asset_failure_location(
    scene: &SceneDocument,
    supplemental: &crate::SupplementalAssets,
    file_id: &str,
    authority: asset::Authority,
) -> Result<(String, u64, u64, String), std::collections::TryReserveError> {
    match authority {
        asset::Authority::Scene => {
            let source_index = (0..scene.file_record_count())
                .find(|index| {
                    scene
                        .file_record_at(*index)
                        .is_some_and(|record| record.key() == file_id)
                })
                .unwrap_or(usize::MAX);
            let record_pointer = crate::scene::pointer_child("/files", file_id)?;
            let data_position = scene
                .file_record_at(source_index)
                .and_then(|record| record.member_position("dataURL"))
                .and_then(|position| u64::try_from(position).ok())
                .unwrap_or(u64::MAX);
            Ok((
                crate::scene::pointer_child(&record_pointer, "dataURL")?,
                scene.source_owner_order("files", source_index),
                data_position,
                try_owned(file_id)?,
            ))
        }
        asset::Authority::Supplemental => Ok((
            String::new(),
            supplemental_source_order(scene, supplemental, file_id),
            0,
            try_owned(file_id)?,
        )),
        asset::Authority::Missing => Ok((String::new(), u64::MAX, u64::MAX, try_owned(file_id)?)),
    }
}

pub(super) fn asset_claim_location(
    scene: &SceneDocument,
    supplemental: &crate::SupplementalAssets,
    file_id: &str,
    authority: asset::Authority,
    field: &str,
) -> Result<(String, u64, u64, String), std::collections::TryReserveError> {
    match authority {
        asset::Authority::Scene => {
            let source_index = (0..scene.file_record_count())
                .find(|index| {
                    scene
                        .file_record_at(*index)
                        .is_some_and(|record| record.key() == file_id)
                })
                .unwrap_or(usize::MAX);
            let pointer = crate::scene::pointer_child(
                &crate::scene::pointer_child("/files", file_id)?,
                field,
            )?;
            let source_position = scene
                .file_record_at(source_index)
                .and_then(|record| record.member_position(field))
                .and_then(|position| u64::try_from(position).ok())
                .unwrap_or(u64::MAX);
            Ok((
                pointer,
                scene.source_owner_order("files", source_index),
                source_position,
                try_owned(file_id)?,
            ))
        }
        asset::Authority::Supplemental => Ok((
            String::new(),
            supplemental_source_order(scene, supplemental, file_id),
            0,
            try_owned(file_id)?,
        )),
        asset::Authority::Missing => unreachable!("missing assets have no media claim"),
    }
}

fn supplemental_source_order(
    scene: &SceneDocument,
    supplemental: &crate::SupplementalAssets,
    file_id: &str,
) -> u64 {
    let rank = supplemental
        .iter()
        .position(|(candidate, _)| candidate == file_id)
        .unwrap_or(usize::MAX);
    scene.source_owner_order("files", scene.file_record_count().saturating_add(rank))
}

pub(super) fn relationship_element_target(
    source_index: usize,
    kind: crate::relationships::RelationshipKind,
    relationships: &crate::relationships::ResolvedRelationshipGraph,
) -> Option<usize> {
    let node_index = relationships.node_position(source_index)?;
    relationships
        .outgoing_edges(node_index)
        .iter()
        .find_map(|edge| {
            if edge.kind != kind {
                return None;
            }
            let crate::relationships::RelationshipTarget::Element(target) = edge.target else {
                return None;
            };
            relationships
                .nodes
                .get(target)
                .map(|node| node.source_index)
        })
}

#[derive(Debug)]
pub(super) enum ResolveSceneError {
    Memory,
    FontUnavailable(super::FontUnavailable),
    InvalidFallbackFont,
    InvalidMandatoryNotice,
    InvalidColor,
    UnboundedGeometry,
    /// The glyph outlines laid out so far, once they exceed the total
    /// geometry limit, and the element whose layout crossed it.
    TextOutlines {
        geometry_segments: u64,
        element_id: String,
        json_pointer: String,
    },
}

pub(super) fn format_error_code(format: OutputFormat) -> ErrorCode {
    match format {
        OutputFormat::Svg => ErrorCode::ENCODE_SVG_FAILED,
        OutputFormat::Png => ErrorCode::ENCODE_PNG_FAILED,
        OutputFormat::Pdf => ErrorCode::ENCODE_PDF_FAILED,
    }
}

impl From<std::collections::TryReserveError> for ResolveSceneError {
    fn from(_: std::collections::TryReserveError) -> Self {
        Self::Memory
    }
}

/// The scene-level failure a label, frame name, official font or fallback font
/// error becomes. `mandatory_notices_json` is the one caller that maps
/// `InvalidFont` elsewhere, to `InvalidMandatoryNotice`, so this stays a named
/// function rather than a `From` impl `?` would apply there too.
pub(super) fn label_error(error: LabelError) -> ResolveSceneError {
    match error {
        LabelError::Memory => ResolveSceneError::Memory,
        LabelError::InvalidFont => ResolveSceneError::InvalidFallbackFont,
        LabelError::FontUnavailable(font) => ResolveSceneError::FontUnavailable(font),
    }
}

/// What every element's resolution reads once the scene's bounds are settled:
/// the request, the relationship graph, the effective frame rendering, the
/// semantic frames that clip and fade their children, the bound-text masks,
/// the canvas background, the CJK subset trigger's outcome, and the layout
/// origin every canvas placement subtracts. Per-element state — the frame
/// clips and opacity the loop computes first, and the official fonts the text
/// and label resolutions load — travels beside it.
struct ResolveContext<'a> {
    request: &'a RenderRequest,
    relationships: &'a ResolvedRelationshipGraph,
    frame_rendering: EffectiveFrameRendering,
    semantic_frames: &'a [SemanticFrameSource],
    bound_text_masks: &'a [BoundTextMaskSource],
    background_color: CanonicalColor,
    cjk_subset_trigger: CjkSubsetTrigger,
    min_x: f64,
    min_y: f64,
}

impl ResolveContext<'_> {
    /// Canvas placement of a source box whose top-left corner is `(x, y)`: the
    /// scene origin moves to the padding, and the rotation is about the given
    /// center in the box's own space.
    fn transform(
        &self,
        x: f64,
        y: f64,
        angle_radians: f64,
        center_x: f64,
        center_y: f64,
    ) -> ResolvedTransform {
        ResolvedTransform {
            translation_x: x - self.min_x + self.request.padding(),
            translation_y: y - self.min_y + self.request.padding(),
            angle_radians,
            center_x,
            center_y,
        }
    }

    /// The placement of an element rotated about the center of its own box.
    fn box_transform(&self, source: &NormalizedElement) -> ResolvedTransform {
        self.transform(
            source.x,
            source.y,
            source.angle,
            source.width / 2.0,
            source.height / 2.0,
        )
    }

    /// The clips the frame that owns `source_index` applies to it.
    fn frame_clips(&self, source_index: usize) -> Result<Vec<ResolvedClip>, ResolveSceneError> {
        resolve_frame_clips(
            source_index,
            self.semantic_frames,
            self.relationships,
            self.frame_rendering,
            self.min_x,
            self.min_y,
            self.request.padding(),
        )
    }

    /// The opacity the frame that owns `source_index` multiplies into it.
    fn frame_opacity(&self, source_index: usize) -> f64 {
        resolve_frame_opacity(
            source_index,
            self.semantic_frames,
            self.relationships,
            self.frame_rendering,
        )
    }

    /// Both at once, clips first: what the element's parent frame imposes on
    /// it, resolved before the element's kind is.
    fn frame_membership(&self, source_index: usize) -> Result<FrameMembership, ResolveSceneError> {
        Ok(FrameMembership {
            clips: self.frame_clips(source_index)?,
            opacity: self.frame_opacity(source_index),
        })
    }
}

/// What the frame an element belongs to imposes on it: the clips of the frame's
/// outline and the frame's opacity, which multiplies the element's own. Resolved
/// before the element's kind is, for every element alike, so the clip's
/// allocation fails in the same place whatever the element turns out to be.
struct FrameMembership {
    clips: Vec<ResolvedClip>,
    opacity: f64,
}

pub(super) type EmbeddableLabel = (ResolvedText, CanonicalColor, f64);

#[derive(Default)]
struct PreparedLabels {
    embeddables: Vec<(usize, EmbeddableLabel)>,
    fallbacks: Vec<(usize, ResolvedLabel)>,
}

impl PreparedLabels {
    fn check(
        &self,
        index: usize,
        range: super::coordinates::CoordinateRange,
        transform: ResolvedTransform,
    ) -> Result<(), f64> {
        if let Some((_, (text, _, offset))) = self.embeddables.iter().find(|(i, _)| *i == index) {
            range.offset_text(text, *offset, transform)?;
        }
        if let Some((_, label)) = self.fallbacks.iter().find(|(i, _)| *i == index) {
            range.label(label, transform)?;
        }
        Ok(())
    }
    fn take_embeddable(&mut self, index: usize) -> Option<EmbeddableLabel> {
        self.embeddables
            .iter()
            .position(|(i, _)| *i == index)
            .map(|i| self.embeddables.remove(i).1)
    }
    fn take_fallback(&mut self, index: usize) -> Option<ResolvedLabel> {
        self.fallbacks
            .iter()
            .position(|(i, _)| *i == index)
            .map(|i| self.fallbacks.remove(i).1)
    }
}

impl ResolvedScene {
    pub(super) fn empty(
        background_color: &str,
        request: &RenderRequest,
        viewport: Option<FrameViewport>,
    ) -> Result<Self, ResolveSceneError> {
        let (logical_width, logical_height) = viewport.map_or_else(
            || {
                let logical_size = request.padding() * 2.0;
                (logical_size.max(1.0), logical_size.max(1.0))
            },
            |viewport| {
                (
                    viewport.max_x - viewport.min_x + request.padding() * 2.0,
                    viewport.max_y - viewport.min_y + request.padding() * 2.0,
                )
            },
        );
        if !logical_width.is_finite() || !logical_height.is_finite() {
            return Err(ResolveSceneError::UnboundedGeometry);
        }
        Ok(Self {
            coordinate_omissions: Vec::new(),
            logical_width: logical_width.max(1.0),
            logical_height: logical_height.max(1.0),
            include_background: request.include_background(),
            background_color: color::parse_css(background_color)
                .ok_or(ResolveSceneError::InvalidColor)?
                .lowered(request.color_lowering()),
            fallback_colors: FallbackColors::lowered(request.color_lowering()),
            elements: Vec::new(),
            fallback_font_woff2: None,
            fallback_font_sfnt: None,
            official_fonts: Vec::new(),
            mandatory_notices: None,
            cjk_subset_trigger: CjkSubsetTrigger::default(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn elements(
        mut outlines: Vec<(NormalizedElement, Option<GeneratedGeometry>)>,
        images: ResolvedImageInputs,
        mut fallbacks: Vec<SurfaceFallback>,
        relationships: &ResolvedRelationshipGraph,
        frame_rendering: EffectiveFrameRendering,
        background_color: &str,
        request: &RenderRequest,
        viewport: Option<FrameViewport>,
        recoveries: &mut Vec<admission::CoordinateOmission>,
        cjk_subset_trigger: CjkSubsetTrigger,
    ) -> Result<Option<Self>, ResolveSceneError> {
        if outlines.is_empty() && fallbacks.is_empty() {
            return Ok(None);
        }

        let text_placements = resolve_bound_text_placements(&outlines, relationships)?;
        let outline_positions = SourcePositions::new(
            outlines
                .iter()
                .map(|(element, _)| element.source.source_index()),
        )?;
        for placement in text_placements {
            let Some((text, _)) = outline_positions
                .first(placement.source_index)
                .and_then(|position| outlines.get_mut(position))
            else {
                continue;
            };
            text.x = placement.x;
            text.y = placement.y;
            text.angle = placement.angle;
        }

        // The reference synthesizes every frame's name as a text element before
        // it measures the scene, so the names take part in the bounds and are
        // shaped before them.
        let shows_frame_names =
            frame_rendering.enabled && frame_rendering.name && frame_rendering.emit_frame_elements;
        let has_frames = outlines.iter().any(|(source, _)| {
            matches!(
                source.kind,
                NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
            )
        });
        let mut font_loader = crate::fonts::FontLoader::default();
        let fallback_font = if (shows_frame_names && has_frames)
            || fallbacks.iter().any(|fallback| fallback.paint)
        {
            Some(FallbackFont::load_with_loader(&mut font_loader).map_err(label_error)?)
        } else {
            None
        };
        let mut frame_names = if shows_frame_names {
            resolve_frame_names(&outlines, fallback_font.as_ref())?
        } else {
            Vec::new()
        };

        // Shape text once, in source space, before any occurrence can set the
        // export origin. Rejected layouts remain available for paint-ordered
        // resource preflight, but cannot displace otherwise valid siblings.
        let semantic_frames = semantic_frame_sources(&outlines)?;
        let mut official_fonts = Vec::<OfficialFont>::new();
        let mut texts = Vec::new();
        texts.try_reserve(outlines.len())?;
        let source_context = ResolveContext {
            request,
            relationships,
            frame_rendering,
            semantic_frames: &semantic_frames,
            bound_text_masks: &[],
            background_color: color::parse_css(background_color)
                .ok_or(ResolveSceneError::InvalidColor)?,
            cjk_subset_trigger,
            min_x: request.padding(),
            min_y: request.padding(),
        };
        let mut labels = PreparedLabels::default();
        labels.embeddables.try_reserve(outlines.len())?;
        labels.fallbacks.try_reserve(fallbacks.len())?;
        // Every text keeps its glyph outlines from layout until accounting,
        // which charges them to the total geometry limit unless it omits the
        // text. Text over the per-element limit keeps none
        // (`TextLayout::outline_limit`); once the kept outlines pass the total
        // limit, layout stops instead of holding more of them.
        let outline_ceiling = request.resource_limits().max_total_geometry_segments();
        let mut kept_outlines = 0_u64;
        let mut keep_outlines = |text: &ResolvedText, source: &NormalizedElementSource| {
            kept_outlines = kept_outlines.saturating_add(text.kept_outline_segments());
            if kept_outlines > outline_ceiling {
                Err(ResolveSceneError::TextOutlines {
                    geometry_segments: kept_outlines,
                    element_id: try_owned(source.id())?,
                    json_pointer: try_owned(source.pointer())?,
                })
            } else {
                Ok(())
            }
        };
        for (source, geometry) in &outlines {
            if matches!(source.kind, NormalizedElementKind::Text(_)) {
                let membership = source_context.frame_membership(source.source.source_index())?;
                if let Some(text) = resolve_text(
                    source,
                    &source_context,
                    &mut official_fonts,
                    membership,
                    &mut font_loader,
                )? {
                    keep_outlines(&text.text, &text.source)?;
                    texts.push(text);
                }
            }
            if let NormalizedElementKind::Embeddable { label, .. } = &source.kind {
                if geometry.is_some()
                    && (request.format() != OutputFormat::Png
                        || source.width != 0.0 && source.height != 0.0)
                {
                    let label = outline::resolve_embeddable_label(
                        source,
                        label,
                        &source_context,
                        &mut official_fonts,
                        &mut font_loader,
                    )?;
                    keep_outlines(&label.0, &source.source)?;
                    labels
                        .embeddables
                        .push((source.source.source_index(), label));
                }
            }
        }
        for fallback in &fallbacks {
            if fallback.paint && !fallback.label.is_empty() {
                if let Some(label) = fallback_font
                    .as_ref()
                    .ok_or(ResolveSceneError::InvalidFallbackFont)?
                    .resolve(&fallback.label, fallback.width, fallback.height)
                    .map_err(label_error)?
                {
                    labels.fallbacks.push((fallback.source_index, label));
                }
            }
        }
        let Some((mut min_x, mut min_y, mut max_x, mut max_y)) = admission::admit_layout(
            &mut outlines,
            &mut fallbacks,
            &mut frame_names,
            relationships,
            frame_rendering,
            viewport,
            request,
            recoveries,
            &mut texts,
            &images,
            &mut labels,
        )?
        else {
            return Ok(None);
        };
        let bound_text_masks = resolve_bound_text_masks(&outlines, relationships)?;
        if min_x == f64::INFINITY
            && min_y == f64::INFINITY
            && max_x == f64::NEG_INFINITY
            && max_y == f64::NEG_INFINITY
        {
            if texts.is_empty() {
                return Self::empty(background_color, request, viewport).map(Some);
            }
            let extent = (1.0 - request.padding() * 2.0).max(0.0);
            (min_x, min_y, max_x, max_y) = (0.0, 0.0, extent, extent);
        }
        let logical_width = max_x - min_x + request.padding() * 2.0;
        let logical_height = max_y - min_y + request.padding() * 2.0;
        if !logical_width.is_finite() || !logical_height.is_finite() {
            return Err(ResolveSceneError::UnboundedGeometry);
        }

        let mut elements = Vec::new();
        elements.try_reserve_exact(
            outlines
                .len()
                .saturating_add(fallbacks.len())
                .saturating_add(texts.len()),
        )?;
        let background_color = color::parse_css(background_color)
            .ok_or(ResolveSceneError::InvalidColor)?
            .lowered(request.color_lowering());
        let semantic_frames = semantic_frame_sources(&outlines)?;
        let fallback_frame_clips = fallback_frame_clip_sources(&fallbacks, frame_rendering)?;
        let mut has_label = false;
        // Each text outline takes its laid-out text. Taking leaves an empty
        // slot rather than shifting the rest, and the slots are keyed by
        // source index, so neither the lookup nor the take scans the texts.
        let text_positions =
            SourcePositions::new(texts.iter().map(|text| text.source.source_index()))?;
        let mut text_slots = Vec::new();
        text_slots.try_reserve_exact(texts.len())?;
        text_slots.extend(texts.into_iter().map(Some));
        let context = ResolveContext {
            request,
            relationships,
            frame_rendering,
            semantic_frames: &semantic_frames,
            bound_text_masks: &bound_text_masks,
            background_color,
            cjk_subset_trigger,
            min_x,
            min_y,
        };
        for (source, geometry) in outlines {
            let membership = context.frame_membership(source.source.source_index())?;
            let prepared_label = labels.take_embeddable(source.source.source_index());
            let element = match &source.kind {
                NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_) => {
                    resolve_frame(source, &context, membership, &mut frame_names)?
                        .map(ResolvedElement::Frame)
                }
                NormalizedElementKind::Text(_) => text_positions
                    .all(source.source.source_index())
                    .find_map(|position| text_slots.get_mut(position)?.take())
                    .map(|mut text| {
                        text.transform = context.box_transform(&source);
                        text.frame_clips = membership.clips;
                        text.opacity = source.opacity / 100.0 * membership.opacity;
                        ResolvedElement::Text(text)
                    }),
                NormalizedElementKind::Image(_) => {
                    resolve_image(source, &images, &context, membership)?
                }
                NormalizedElementKind::Rectangle
                | NormalizedElementKind::Diamond
                | NormalizedElementKind::Ellipse
                | NormalizedElementKind::Linear(_)
                | NormalizedElementKind::Freedraw(_)
                | NormalizedElementKind::Embeddable { .. } => geometry
                    .map(|geometry| {
                        resolve_geometry(source, geometry, &context, prepared_label, membership)
                    })
                    .transpose()?,
            };
            let Some(element) = element else {
                continue;
            };
            if let Err(value) = super::coordinates::resolved(
                &element,
                super::coordinates::CoordinateRange(
                    request.resource_limits().max_coordinate_magnitude(),
                ),
            ) {
                admission::omit_resolved(&element, value, request, recoveries)?;
                continue;
            }
            if let ResolvedElement::Frame(frame) = &element {
                has_label |= frame.name.is_some();
            }
            elements.push(element);
        }
        // Only rejected text remains. Accounting observes its geometry before
        // omitting it, preserving aggregate-fatal precedence and paint order.
        elements.extend(
            text_slots
                .into_iter()
                .flatten()
                .filter(|text| text.coordinate_failure.is_some())
                .map(ResolvedElement::Text),
        );
        for fallback in fallbacks {
            if !fallback.paint {
                continue;
            }
            let prepared_label = labels.take_fallback(fallback.source_index);
            let fallback =
                resolve_fallback(fallback, &context, prepared_label, &fallback_frame_clips)?;
            let element = ResolvedElement::Fallback(fallback);
            if let Err(value) = super::coordinates::resolved(
                &element,
                super::coordinates::CoordinateRange(
                    request.resource_limits().max_coordinate_magnitude(),
                ),
            ) {
                admission::omit_resolved(&element, value, request, recoveries)?;
                continue;
            }
            if let ResolvedElement::Fallback(fallback) = &element {
                has_label |= fallback.label.is_some();
            }
            elements.push(element);
        }
        elements.sort_by_key(ResolvedElement::paint_order);
        // A label omitted after shaping must not leave a hole in its arrow.
        let mut surviving_containers = Vec::new();
        surviving_containers.try_reserve(elements.len())?;
        for element in &elements {
            if let ResolvedElement::Text(text) = element {
                if text.coordinate_failure.is_none() {
                    if let Some(order) = text.container_source_order {
                        surviving_containers.push(order);
                    }
                }
            }
        }
        // Membership only: sorted for a binary search per outline.
        surviving_containers.sort_unstable();
        for element in &mut elements {
            if let ResolvedElement::Outline(outline) = element {
                if surviving_containers
                    .binary_search(&outline.source_order)
                    .is_err()
                {
                    outline.label_mask = None;
                }
            }
        }
        let (fallback_font_woff2, fallback_font_sfnt) = if has_label {
            fallback_font
                .map(FallbackFont::into_parts)
                .map_or((None, None), |(woff2, sfnt)| (Some(woff2), Some(sfnt)))
        } else {
            (None, None)
        };
        prune_unused_official_fonts(&mut elements, &mut official_fonts)?;
        let has_official_text = elements.iter().any(|element| match element {
            ResolvedElement::Text(_) => true,
            ResolvedElement::Outline(outline) => outline.generated_label.is_some(),
            _ => false,
        });
        let resolved_fonts = resolve_fonts(official_fonts)?;
        let mandatory_notices = if has_label || has_official_text {
            Some(mandatory_notices_json(has_label, &resolved_fonts).map_err(
                |error| match error {
                    LabelError::Memory => ResolveSceneError::Memory,
                    LabelError::InvalidFont => ResolveSceneError::InvalidMandatoryNotice,
                    LabelError::FontUnavailable(font) => ResolveSceneError::FontUnavailable(font),
                },
            )?)
        } else {
            None
        };
        Ok(Some(Self {
            coordinate_omissions: Vec::new(),
            logical_width,
            logical_height,
            include_background: request.include_background(),
            background_color,
            fallback_colors: FallbackColors::lowered(request.color_lowering()),
            elements,
            fallback_font_woff2,
            fallback_font_sfnt,
            official_fonts: resolved_fonts,
            mandatory_notices,
            cjk_subset_trigger,
        }))
    }

    pub(super) fn source_count(&self) -> usize {
        self.elements.len()
    }

    pub(super) fn source(&self, index: usize) -> Option<(Option<&str>, &str)> {
        self.elements.get(index).map(|element| match element {
            ResolvedElement::Outline(outline) => {
                (Some(outline.source.id()), outline.source.pointer())
            }
            ResolvedElement::Freedraw(freedraw) => {
                (Some(freedraw.source.id()), freedraw.source.pointer())
            }
            ResolvedElement::Frame(frame) => (Some(frame.source.id()), frame.source.pointer()),
            ResolvedElement::Text(text) => (Some(text.source.id()), text.source.pointer()),
            ResolvedElement::Image(image) => (Some(image.source.id()), image.source.pointer()),
            ResolvedElement::ImageFallback(image) => {
                (Some(image.source.id()), image.source.pointer())
            }
            ResolvedElement::Fallback(fallback) => (
                fallback.source.id.as_deref(),
                fallback.source.pointer.as_str(),
            ),
        })
    }

    #[cfg(any(feature = "png", feature = "pdf"))]
    pub(super) fn raster_or_pdf_scale(&self, sizing: crate::Sizing) -> f64 {
        use crate::Sizing;

        let largest_dimension = self.logical_width.max(self.logical_height);
        match sizing {
            Sizing::Scale(scale) => scale,
            Sizing::MaxDimension(maximum) if maximum < largest_dimension => {
                maximum / largest_dimension
            }
            Sizing::MaxDimension(_) => 1.0,
        }
    }
}

#[derive(Debug)]
pub(super) enum ResolvedElement {
    Outline(ResolvedOutline),
    Freedraw(ResolvedFreedraw),
    Frame(ResolvedFrame),
    Text(ResolvedTextElement),
    Image(ResolvedImage),
    ImageFallback(ResolvedImageFallback),
    Fallback(ResolvedFallback),
}

impl ResolvedElement {
    pub(super) fn source_order(&self) -> u64 {
        match self {
            Self::Outline(outline) => outline.source_order,
            Self::Freedraw(freedraw) => freedraw.source_order,
            Self::Frame(frame) => frame.source_order,
            Self::Text(text) => text.source_order,
            Self::Image(image) => image.source_order,
            Self::ImageFallback(image) => image.source_order,
            Self::Fallback(fallback) => fallback.source_order,
        }
    }

    pub(super) fn paint_order(&self) -> (bool, u64, u8) {
        match self {
            Self::Text(text) => text
                .container_source_order
                .map_or((false, text.source_order, 0), |order| (false, order, 1)),
            Self::Outline(outline) => (outline.final_pass, outline.source_order, 0),
            _ => (false, self.source_order(), 0),
        }
    }
}

/// A resolved image asset. Resolution is the same in every build, so the
/// fields only one format's encoder reads stay in builds without that format.
#[derive(Debug)]
pub(super) struct ResolvedImageAsset {
    pub(super) identity: [u8; 32],
    pub(super) source_bytes: std::sync::Arc<Vec<u8>>,
    pub(super) media_type: &'static str,
    pub(super) width: u32,
    pub(super) height: u32,
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(super) rgba: Option<std::sync::Arc<Vec<u8>>>,
    #[cfg_attr(not(feature = "png"), allow(dead_code))]
    pub(super) premultiplied_rgba: Option<std::sync::Arc<Vec<u8>>>,
    pub(super) sanitized_svg: Option<std::sync::Arc<Vec<u8>>>,
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(super) svg_effects: Vec<asset::svg::Effect>,
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(super) svg_fallback_pointer: Option<String>,
}

#[derive(Debug)]
pub(super) struct ResolvedImageInputs {
    pub(super) assets: std::collections::BTreeMap<usize, std::sync::Arc<ResolvedImageAsset>>,
    pub(super) fallbacks: Vec<(usize, crate::normalization::NormalizedImageStatus)>,
}

#[derive(Debug)]
pub(super) struct ResolvedImage {
    source_order: u64,
    pub(super) source: NormalizedElementSource,
    pub(super) asset: std::sync::Arc<ResolvedImageAsset>,
    /// The reference's dark-mode filter chain this image passes through, or
    /// `None` in light mode.
    pub(super) dark_mode_filter: Option<DarkModeImageFilter>,
    pub(super) frame_clips: Vec<ResolvedClip>,
    /// The rounded-corner clip of an image with a roundness record, in the
    /// element's own space before crop, flip, and rotation; `None` without one.
    pub(super) corner_clip: Option<ResolvedImageCornerClip>,
    pub(super) transform: ResolvedTransform,
    pub(super) opacity: f64,
    /// The crop in the asset's pixel grid, the canvas `drawImage` source
    /// rectangle PNG draws from.
    pub(super) source_rect: ResolvedImageSourceRect,
    /// The reference SVG export's viewport and content placement, which SVG
    /// and PDF draw through.
    pub(super) projection: ResolvedImageProjection,
    pub(super) scale: [i8; 2],
    pub(super) link: Option<String>,
    /// The element's unrounded size: its flip axis, rotation center, corner
    /// clip, link rectangle, and the canvas rectangle PNG draws into.
    pub(super) width: f64,
    pub(super) height: f64,
}

/// The reference export's dark-mode filters an image passes through, resolved
/// once from the color mode and the asset kind so each format lowers the chain
/// its authority applies (spec section 8.2). The reference SVG puts
/// `IMAGE_INVERT_FILTER` on a raster image's `<use>` under its root
/// `THEME_FILTER` and only the root filter on a nested-SVG image; the reference
/// canvas export resets its filter for a raster image and keeps the theme
/// filter for a nested-SVG image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DarkModeImageFilter {
    /// `IMAGE_INVERT_FILTER` followed by `THEME_FILTER`. SVG references the
    /// image filter under its root theme filter and PDF, whose authority is
    /// official SVG, lowers both into the pixels; PNG follows the canvas and
    /// leaves a raster image's pixels alone.
    RasterImage,
    /// `THEME_FILTER` alone. SVG applies it through its root filter, PNG
    /// lowers it into the pixels it rasterizes, and PDF lowers it into the
    /// nested SVG's content before converting it (#154).
    NestedSvgImage,
}

impl DarkModeImageFilter {
    fn for_image(mode: crate::ColorMode, asset: &ResolvedImageAsset) -> Option<Self> {
        match mode {
            crate::ColorMode::Light => None,
            crate::ColorMode::Dark if asset.sanitized_svg.is_some() => Some(Self::NestedSvgImage),
            crate::ColorMode::Dark => Some(Self::RasterImage),
        }
    }
}

/// The clip the reference applies to an image element with a `roundness`
/// record: its rectangle with `getCornerRadius(min(width, height))` corners,
/// which its SVG defines as a `<clipPath>` holding a `<rect rx ry>` and its
/// canvas paints through `roundRect`. Like a frame clip it carries both the
/// radius for the SVG rectangle and the same outline as a path for the
/// backends that rasterize or emit one.
#[derive(Debug)]
pub(super) struct ResolvedImageCornerClip {
    pub(super) radius: f64,
    pub(super) path: Path,
}

impl ResolvedImageCornerClip {
    /// The corner clip of an image element, or `None` when it has no roundness
    /// record or its radius is not positive: a zero radius clips nothing the
    /// image's rectangle does not already clip.
    pub(super) fn for_image(
        width: f64,
        height: f64,
        roundness: Option<crate::normalization::NormalizedRoundness>,
    ) -> Result<Option<Self>, ResolveSceneError> {
        let radius = image_corner_radius(width, height, roundness);
        if !radius.is_finite() || radius <= 0.0 {
            return Ok(None);
        }
        Ok(Some(Self {
            radius,
            path: rounded_rect_path(width, height, radius)?,
        }))
    }
}

#[derive(Debug)]
pub(super) struct ResolvedImageFallback {
    source_order: u64,
    pub(super) source: NormalizedElementSource,
    /// Only PNG paints the status's placeholder and its opacity.
    #[cfg_attr(not(feature = "png"), allow(dead_code))]
    pub(super) status: crate::normalization::NormalizedImageStatus,
    pub(super) frame_clips: Vec<ResolvedClip>,
    pub(super) transform: ResolvedTransform,
    #[cfg_attr(not(feature = "png"), allow(dead_code))]
    pub(super) opacity: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

/// How the reference SVG export places an image, which SVG and PDF (whose
/// visual authority is the official SVG, spec section 8.6) reproduce: the
/// whole asset is drawn into the content rectangle and clipped to the
/// viewport, both in the element's own space before its `scale` flip about the
/// element's unrounded center and its rotation.
///
/// The viewport is `Math.round(width)` by `Math.round(height)`, the size of
/// the reference's `<use>` and of its crop `<mask>`; it is the only image value
/// that rounds (spec section 8.6). An uncropped image fills the rounded
/// viewport, as the reference's `<symbol>` image of `width="100%"` does; a
/// cropped image keeps the reference's unrounded `getUncroppedWidthAndHeight`
/// size at the negative `normalizedCrop` offset of its group translation, and
/// the viewport clips it.
///
/// PNG follows the canvas, which draws the crop into the element's unrounded
/// rectangle (`drawImage(..., 0, 0, element.width, element.height)`), and
/// does not use this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolvedImageProjection {
    pub(super) viewport_width: f64,
    pub(super) viewport_height: f64,
    /// A cropped image's placement; `None` for an uncropped image, whose
    /// content is the viewport itself.
    pub(super) crop: Option<ResolvedImageCropPlacement>,
}

/// A cropped image's placement in the reference export: the whole image at
/// its unrounded `getUncroppedWidthAndHeight` size, shifted up and left by the
/// `normalizedCrop` offset so the crop's top-left corner sits at the element's
/// origin. Neither value inherits the viewport's rounding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolvedImageCropPlacement {
    pub(super) offset_x: f64,
    pub(super) offset_y: f64,
    pub(super) uncropped_width: f64,
    pub(super) uncropped_height: f64,
}

impl ResolvedImageProjection {
    /// The projection of an image element of the given unrounded size and
    /// normalized crop record, if it has one.
    pub(super) fn for_image(
        width: f64,
        height: f64,
        crop: Option<crate::normalization::NormalizedImageCrop>,
    ) -> Self {
        Self {
            viewport_width: ecmascript_round(width),
            viewport_height: ecmascript_round(height),
            crop: crop.map(|crop| {
                // The reference's arithmetic, operation for operation, so the
                // emitted numbers are its numbers: `getUncroppedWidthAndHeight`
                // divides the element size by the crop's share of the natural
                // size, and the normalized crop offset divides the crop offset
                // by the natural-to-uncropped ratio.
                let uncropped_width = width / (crop.width / crop.natural_width);
                let uncropped_height = height / (crop.height / crop.natural_height);
                ResolvedImageCropPlacement {
                    offset_x: crop.x / (crop.natural_width / uncropped_width),
                    offset_y: crop.y / (crop.natural_height / uncropped_height),
                    uncropped_width,
                    uncropped_height,
                }
            }),
        }
    }

    /// The rectangle the whole asset is drawn into, as `(x, y, width, height)`
    /// in the element's space before its flip.
    pub(super) fn content_rect(&self) -> (f64, f64, f64, f64) {
        match self.crop {
            None => (0.0, 0.0, self.viewport_width, self.viewport_height),
            Some(crop) => (
                -crop.offset_x,
                -crop.offset_y,
                crop.uncropped_width,
                crop.uncropped_height,
            ),
        }
    }

    /// The reference `<use>` viewport, as `(x, y, width, height)` in the
    /// element's space before its flip: from the content's origin to the
    /// rounded viewport's far edges, so `width + normalizedCropX` wide as the
    /// reference sizes it, and the viewport itself for an uncropped image.
    pub(super) fn viewport_rect(&self) -> (f64, f64, f64, f64) {
        let (content_x, content_y, _, _) = self.content_rect();
        (
            content_x,
            content_y,
            self.viewport_width - content_x,
            self.viewport_height - content_y,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolvedImageSourceRect {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

impl ResolvedImageSourceRect {
    pub(super) fn for_image(
        asset: &ResolvedImageAsset,
        crop: Option<crate::normalization::NormalizedImageCrop>,
    ) -> Self {
        crop.map_or(
            Self {
                x: 0.0,
                y: 0.0,
                width: f64::from(asset.width),
                height: f64::from(asset.height),
            },
            |crop| Self {
                x: crop.x * f64::from(asset.width) / crop.natural_width,
                y: crop.y * f64::from(asset.height) / crop.natural_height,
                width: crop.width * f64::from(asset.width) / crop.natural_width,
                height: crop.height * f64::from(asset.height) / crop.natural_height,
            },
        )
    }
}

#[derive(Debug)]
pub(super) struct ResolvedTextElement {
    pub(super) source_order: u64,
    pub(super) source: NormalizedElementSource,
    pub(super) font_family: i32,
    pub(super) original_text: String,
    pub(super) text: ResolvedText,
    pub(super) frame_clips: Vec<ResolvedClip>,
    pub(super) transform: ResolvedTransform,
    pub(super) color: CanonicalColor,
    pub(super) opacity: f64,
    pub(super) render: bool,
    pub(super) coordinate_failure: Option<f64>,
    pub(super) base_element_work_units: u64,
    container_source_order: Option<u64>,
    pub(super) link: Option<String>,
    pub(super) width: f64,
    pub(super) height: f64,
}

#[derive(Debug)]
pub(super) struct ResolvedOutline {
    pub(super) source_order: u64,
    pub(super) source: NormalizedElementSource,
    pub(super) base_element_work_units: u64,
    pub(super) drawing: rough::Drawing,
    pub(super) frame_clips: Vec<ResolvedClip>,
    pub(super) transform: ResolvedTransform,
    pub(super) stroke_color: CanonicalColor,
    pub(super) fill_color: Option<CanonicalColor>,
    pub(super) stroke_width: f64,
    pub(super) stroke_style: StrokeStyle,
    pub(super) source_stroke_width: f64,
    pub(super) opacity: f64,
    pub(super) element_opacity: f64,
    pub(super) background_color: CanonicalColor,
    pub(super) label_mask: Option<ResolvedLabelMask>,
    pub(super) link: Option<String>,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) generated_label: Option<ResolvedText>,
    pub(super) generated_label_color: Option<CanonicalColor>,
    pub(super) generated_label_offset_y: f64,
    final_pass: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolvedLabelMask {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

#[derive(Debug)]
pub(super) struct ResolvedFreedraw {
    source_order: u64,
    pub(super) source: NormalizedElementSource,
    pub(super) path: freedraw::Path,
    pub(super) background_fill: Option<rough::Fill>,
    pub(super) frame_clips: Vec<ResolvedClip>,
    pub(super) transform: ResolvedTransform,
    pub(super) color: CanonicalColor,
    pub(super) background_color: Option<CanonicalColor>,
    pub(super) source_stroke_width: f64,
    pub(super) opacity: f64,
    pub(super) link: Option<String>,
    pub(super) width: f64,
    pub(super) height: f64,
}

#[derive(Debug)]
pub(super) struct ResolvedFrame {
    pub(super) source_order: u64,
    pub(super) source: NormalizedElementSource,
    pub(super) name: Option<ResolvedFrameName>,
    pub(super) outline: Option<ResolvedFrameOutline>,
    pub(super) frame_clips: Vec<ResolvedClip>,
    pub(super) transform: ResolvedTransform,
    pub(super) opacity: f64,
    pub(super) link: Option<String>,
    pub(super) width: f64,
    pub(super) height: f64,
}

/// A frame's outline in a scene export: the frame rectangle with
/// `FRAME_STYLE.radius` corners, stroked `FRAME_STYLE.strokeWidth` wide in the
/// reference's outline colors, lowered into the color mode like every other
/// color the reference paints under its dark-mode filter.
#[derive(Debug)]
pub(super) struct ResolvedFrameOutline {
    pub(super) path: Path,
    /// `FRAME_STYLE.strokeColor` for the mode: the stroke the reference's SVG
    /// export gives every frame kind, which SVG and PDF paint.
    pub(super) stroke_color: CanonicalColor,
    /// The stroke the reference's canvas export gives the frame for the mode,
    /// which PNG paints: `FRAME_STYLE.strokeColor` for an ordinary frame and
    /// the theme's magic-frame color for a magic frame.
    #[cfg_attr(not(feature = "png"), allow(dead_code))]
    pub(super) canvas_stroke_color: CanonicalColor,
}

impl ResolvedFrameOutline {
    fn new(
        width: f64,
        height: f64,
        magic: bool,
        theme: crate::ColorMode,
        lowering: ColorLowering,
    ) -> Result<Self, ResolveSceneError> {
        let stroke_color = FRAME_OUTLINE_STROKE_COLOR.lowered(lowering);
        Ok(Self {
            path: resolved_frame_outline(width, height)?,
            stroke_color,
            // Only PNG paints the canvas color, and PNG lowers exactly when
            // the theme is dark.
            canvas_stroke_color: if magic {
                magic_frame_canvas_outline_stroke_color(theme)
            } else {
                stroke_color
            },
        })
    }
}

/// A frame's visible name as the reference export renders it: a synthesized
/// text element (`addFrameLabelsAsTextElements`) rather than a decoration of
/// the frame. It has its own unrotated placement above the frame's top-left
/// corner, full opacity, no link, and no frame clip, whatever the frame has.
#[derive(Debug)]
pub(super) struct ResolvedFrameName {
    pub(super) label: ResolvedLabel,
    /// Canvas placement of the label box's top-left corner; the angle is 0.
    pub(super) transform: ResolvedTransform,
    /// `FRAME_STYLE.nameColorLightTheme` or `nameColorDarkTheme` for the mode.
    pub(super) color: CanonicalColor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StrokeStyle {
    Solid,
    Dashed,
    Dotted,
}

pub(super) fn decoration_fill_color(
    decoration: &rough::Decoration,
    stroke: CanonicalColor,
    background: CanonicalColor,
) -> CanonicalColor {
    match decoration.fill_paint {
        rough::DecorationFillPaint::Stroke => stroke,
        rough::DecorationFillPaint::Background => background,
    }
}

/// The stroke style an arrowhead is drawn with and the stroke width its dash
/// gap is derived from, from the `strokeLineDash` the head was generated
/// under (#171): the element's own dashed or dotted pattern reads the
/// element's stroke width, and the open heads' shortened dotted pattern
/// `[1.5, 4 + strokeWidth]` is the dotted pattern of a stroke two narrower.
pub(super) fn decoration_stroke_style(
    decoration: &rough::Decoration,
    source_stroke_width: f64,
) -> (StrokeStyle, f64) {
    match decoration.dash {
        rough::DecorationDash::Solid => (StrokeStyle::Solid, source_stroke_width),
        rough::DecorationDash::Dashed => (StrokeStyle::Dashed, source_stroke_width),
        rough::DecorationDash::Dotted => (StrokeStyle::Dotted, source_stroke_width),
        rough::DecorationDash::DottedHead => (StrokeStyle::Dotted, source_stroke_width - 2.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FallbackColors {
    pub(super) fill: CanonicalColor,
    pub(super) stroke: CanonicalColor,
    pub(super) label: CanonicalColor,
}

impl FallbackColors {
    pub(super) fn lowered(lowering: ColorLowering) -> Self {
        Self {
            fill: CanonicalColor::from_rgb8(0xf1, 0xf3, 0xf5).lowered(lowering),
            stroke: CanonicalColor::from_rgb8(0x86, 0x8e, 0x96).lowered(lowering),
            label: CanonicalColor::from_rgb8(0x49, 0x50, 0x57).lowered(lowering),
        }
    }
}

#[derive(Debug)]
pub(super) struct ResolvedFallback {
    pub(super) source_order: u64,
    pub(super) source: FallbackSource,
    pub(super) label: Option<ResolvedLabel>,
    pub(super) link: Option<String>,
    pub(super) frame_clips: Vec<ResolvedClip>,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) transform: ResolvedTransform,
    pub(super) opacity: f64,
    pub(super) render: bool,
}

#[derive(Debug)]
pub(super) struct SemanticFrameSource {
    pub(super) source_index: usize,
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) angle: f64,
    pub(super) opacity: f64,
}

#[derive(Debug)]
pub(super) struct ResolvedClip {
    /// Source index of the frame whose outline this clip reproduces. Every
    /// element clipped by the same frame shares one clip identity, which lets
    /// the PNG encoder build and charge that frame's alpha mask once and the
    /// SVG define it once.
    pub(super) frame_source_index: usize,
    /// The clip outline as a path, for the backends that rasterize or emit it
    /// as one.
    pub(super) path: Path,
    /// The same outline as the frame rectangle the reference SVG defines its
    /// `<clipPath>` with: the frame's size and `FRAME_STYLE.radius` for a scene
    /// export, or no radius for the rectangular frame-export clip.
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) radius: f64,
    pub(super) transform: ResolvedTransform,
}

pub(super) fn frame_clip_delta(clips: &[ResolvedClip]) -> ResourceDelta {
    let geometry_segments = clips.iter().fold(0_u64, |sum, clip| {
        sum.saturating_add(clip.path.operations.len() as u64)
    });
    ResourceDelta {
        geometry_segments,
        total_work_units: geometry_segments.saturating_mul(32),
        ..ResourceDelta::default()
    }
}

impl ResolvedClip {
    /// The clip one frame applies to its children: the frame's rectangle with
    /// `FRAME_STYLE.radius` corners in a scene export, or the reference's
    /// rectangular target clip in a frame export.
    pub(super) fn for_frame(
        frame_source_index: usize,
        width: f64,
        height: f64,
        rounded: bool,
        transform: ResolvedTransform,
    ) -> Result<Self, ResolveSceneError> {
        let (path, radius) = if rounded {
            (resolved_frame_outline(width, height)?, FRAME_RADIUS)
        } else {
            (resolved_frame_rect(width, height)?, 0.0)
        };
        Ok(Self {
            frame_source_index,
            path,
            width,
            height,
            radius,
            transform,
        })
    }
}

impl ResolvedFallback {
    pub(super) fn geometry_command_count(&self) -> u64 {
        let visual_commands: u64 = if self.width >= 2.0 && self.height >= 2.0 {
            9
        } else {
            5
        };
        visual_commands.saturating_add(self.label.as_ref().map_or(0, |label| {
            u64::try_from(label.path.operations.len()).unwrap_or(u64::MAX)
        }))
    }
}

#[derive(Debug)]
pub(super) struct FallbackSource {
    pub(super) id: Option<String>,
    pub(super) pointer: String,
}

#[derive(Debug)]
pub(super) struct SemanticWorkingRecord {
    pub(super) source_order: u64,
    pub(super) event: &'static str,
    pub(super) element_id: Option<String>,
    pub(super) local_owner: Option<String>,
    pub(super) json_pointer: String,
    pub(super) bytes: u64,
}

impl SemanticWorkingRecord {
    /// Source order alone has ties (geometry, text runs and relationship edges
    /// share an owner). Unstable sorting of those ties varies with target layout.
    /// Compare every released-record field so only indistinguishable events tie,
    /// retaining an allocation-free sort for the bounded accounting path.
    pub(super) fn release_order(&self, other: &Self) -> std::cmp::Ordering {
        (
            self.source_order,
            &self.json_pointer,
            self.event,
            &self.element_id,
            &self.local_owner,
            self.bytes,
        )
            .cmp(&(
                other.source_order,
                &other.json_pointer,
                other.event,
                &other.element_id,
                &other.local_owner,
                other.bytes,
            ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolvedTransform {
    pub(super) translation_x: f64,
    pub(super) translation_y: f64,
    pub(super) angle_radians: f64,
    pub(super) center_x: f64,
    pub(super) center_y: f64,
}

/// A local semantic resource ceiling one element crossed, and how the
/// diagnostic names it: the limit fixes the code, field and value kind; the
/// crossing adds the configured and attempted values, the element member the
/// pointer narrows to (or the element itself), and the message.
pub(super) struct ElementLocalCrossing {
    pub(super) limit: LocalLimit,
    pub(super) configured: ResourceValue,
    pub(super) attempted: crate::ResourceMeasurement,
    pub(super) pointer_field: Option<&'static str>,
    pub(super) message: &'static str,
}

impl ElementLocalCrossing {
    /// The diagnostic of an occurrence omitted after this crossing.
    pub(super) fn diagnostic(
        self,
        format: OutputFormat,
        element_id: Option<&str>,
        element_pointer: &str,
    ) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
        self.diagnostic_with_recovery(
            format,
            element_id,
            element_pointer,
            LocalRecovery::Omitted,
            true,
        )
    }

    pub(super) fn diagnostic_with_recovery(
        self,
        format: OutputFormat,
        element_id: Option<&str>,
        element_pointer: &str,
        recovery: LocalRecovery,
        loses_semantics: bool,
    ) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
        let json_pointer = self.pointer_field.map_or_else(
            || try_owned(element_pointer),
            |field| crate::scene::pointer_child(element_pointer, field),
        )?;
        PendingDiagnostic::new(
            DiagnosticKind::LocalLimitCrossed {
                limit: self.limit,
                configured: self.configured,
                attempted: self.attempted,
                recovery,
            },
            format,
            if loses_semantics && format != OutputFormat::Png {
                &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic]
            } else {
                &[DiagnosticEffect::Visual]
            },
            element_id,
            &json_pointer,
            self.message,
        )
    }
}

pub(super) fn outline_local_crossing(
    source: &NormalizedElement,
    request: &RenderRequest,
    geometry_segments: u64,
    element_work_units: u64,
) -> Option<ElementLocalCrossing> {
    if let Some(crossing) = coordinate_local_crossing(source, request) {
        return Some(crossing);
    }

    let geometry_limit = u64::from(
        request
            .resource_limits()
            .max_geometry_segments_per_element(),
    );
    if geometry_segments > geometry_limit {
        return Some(ElementLocalCrossing {
            limit: LocalLimit::GeometrySegmentsPerElement,
            configured: ResourceValue::Integer(geometry_limit),
            attempted: crate::Measurement::Attempted {
                value: ResourceValue::Integer(geometry_segments),
            },
            pointer_field: None,
            message: "outline omitted because its geometry exceeds its limit",
        });
    }
    let work_limit = request.resource_limits().max_element_work_units();
    (element_work_units > work_limit).then_some(ElementLocalCrossing {
        limit: LocalLimit::ElementWorkUnits,
        configured: ResourceValue::Integer(work_limit),
        attempted: crate::Measurement::Attempted {
            value: ResourceValue::Integer(element_work_units),
        },
        pointer_field: None,
        message: "outline omitted because its semantic work exceeds its limit",
    })
}

pub(super) fn coordinate_local_crossing(
    source: &NormalizedElement,
    request: &RenderRequest,
) -> Option<ElementLocalCrossing> {
    let coordinate_limit = request.resource_limits().max_coordinate_magnitude();
    for (value, pointer_field) in [
        (source.x, "x"),
        (source.y, "y"),
        (source.width, "width"),
        (source.height, "height"),
    ] {
        let magnitude = value.abs();
        if magnitude > coordinate_limit {
            return Some(ElementLocalCrossing {
                limit: LocalLimit::CoordinateMagnitude,
                configured: ResourceValue::Finite(FiniteValue::new(coordinate_limit)?),
                attempted: crate::Measurement::Attempted {
                    value: ResourceValue::Finite(FiniteValue::new(magnitude)?),
                },
                pointer_field: Some(pointer_field),
                message: "outline omitted because a coordinate exceeds its limit",
            });
        }
    }
    let points = match &source.kind {
        NormalizedElementKind::Linear(linear) => Some(linear.points.as_slice()),
        NormalizedElementKind::Freedraw(freedraw) => Some(freedraw.points.as_slice()),
        _ => None,
    };
    if let Some(points) = points {
        for point in points {
            for value in point {
                let magnitude = value.abs();
                if magnitude > coordinate_limit {
                    return Some(ElementLocalCrossing {
                        limit: LocalLimit::CoordinateMagnitude,
                        configured: ResourceValue::Finite(FiniteValue::new(coordinate_limit)?),
                        attempted: crate::Measurement::Attempted {
                            value: ResourceValue::Finite(FiniteValue::new(magnitude)?),
                        },
                        pointer_field: Some("points"),
                        message: "outline omitted because a coordinate exceeds its limit",
                    });
                }
            }
        }
    }

    super::coordinates::source_derived(
        source,
        super::coordinates::CoordinateRange(coordinate_limit),
    )
    .err()
    .map(|value| super::coordinates::crossing(value, request))
}

pub(super) fn generation_local_crossing(
    kind: rough::GenerationLimit,
    attempted: u64,
    request: &RenderRequest,
) -> ElementLocalCrossing {
    match kind {
        rough::GenerationLimit::GeometrySegments => ElementLocalCrossing {
            limit: LocalLimit::GeometrySegmentsPerElement,
            configured: ResourceValue::Integer(u64::from(
                request
                    .resource_limits()
                    .max_geometry_segments_per_element(),
            )),
            attempted: crate::Measurement::Attempted {
                value: ResourceValue::Integer(attempted),
            },
            pointer_field: None,
            message: "outline omitted because its geometry exceeds its limit",
        },
        rough::GenerationLimit::WorkUnits => ElementLocalCrossing {
            limit: LocalLimit::ElementWorkUnits,
            configured: ResourceValue::Integer(request.resource_limits().max_element_work_units()),
            attempted: crate::Measurement::Attempted {
                value: ResourceValue::Integer(attempted),
            },
            pointer_field: None,
            message: "outline omitted because its semantic work exceeds its limit",
        },
    }
}

pub(super) fn logical_aligned_bytes(bytes: u64) -> u64 {
    bytes.saturating_add(63) / 64 * 64
}

pub(super) fn fallback_fits_local_limits(
    fallback: &SurfaceFallback,
    limits: &crate::ResourceLimits,
) -> bool {
    let coordinate_limit = limits.max_coordinate_magnitude();
    if super::coordinates::CoordinateRange(coordinate_limit)
        .box_at(
            fallback.width,
            fallback.height,
            ResolvedTransform {
                translation_x: fallback.x,
                translation_y: fallback.y,
                angle_radians: fallback.angle,
                center_x: fallback.width / 2.0,
                center_y: fallback.height / 2.0,
            },
        )
        .is_err()
    {
        return false;
    }
    let geometry_segments = if fallback.width >= 2.0 && fallback.height >= 2.0 {
        9_u64
    } else {
        5
    };
    geometry_segments <= u64::from(limits.max_geometry_segments_per_element())
        && geometry_segments.saturating_mul(32) <= limits.max_element_work_units()
}

pub(super) fn artifact_retention_delta(
    output_bytes: u64,
    codec_emission_work: u64,
) -> ResourceDelta {
    ResourceDelta {
        working_bytes: 512_u64.saturating_add(logical_aligned_bytes(output_bytes)),
        output_bytes,
        total_work_units: codec_emission_work.saturating_add(output_bytes),
        ..ResourceDelta::default()
    }
}
