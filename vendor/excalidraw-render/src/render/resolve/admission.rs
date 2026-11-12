//! Admit layout before bounds become the export origin. Recovery is monotone:
//! each retry removes at least one occurrence, including any dependent clips.

use super::super::bound_text::SourcePositions;
use super::super::coordinates::{self, CoordinateRange};
use super::super::frame_target::{EffectiveFrameRendering, FrameViewport};
use super::super::geometry::{resolved_frame_outline, resolved_frame_rect, GeneratedGeometry};
use super::super::text::{FrameName, ResolvedText, FRAME_NAME_BOX_HEIGHT, FRAME_NAME_OFFSET_Y};
use super::super::try_owned;
use super::{
    bounds, fallback, frame_clip_delta, outline, scene_bounds, PreparedLabels, ResolveSceneError,
    ResolvedElement, ResolvedImageInputs, ResolvedImageProjection, ResolvedImageSourceRect,
    ResolvedTextElement, ResolvedTransform,
};
use crate::diagnostic_registry::LocalRecovery;
use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::outcome::PendingDiagnostic;
use crate::relationships::ResolvedRelationshipGraph;
use crate::resource_accounting::ResourceDelta;
use crate::scene_surface::SurfaceFallback;
use crate::RenderRequest;

#[derive(Debug)]
pub(in crate::render) struct CoordinateOmission {
    pub(in crate::render) source_order: u64,
    pub(in crate::render) id: Option<String>,
    pub(in crate::render) pointer: String,
    pub(in crate::render) diagnostic: Option<PendingDiagnostic>,
    pub(in crate::render) delta: ResourceDelta,
    pub(in crate::render) paint_order: (bool, u64, u8),
}

fn omit(
    id: Option<&str>,
    pointer: &str,
    source_order: u64,
    value: Option<f64>,
    semantic: bool,
    request: &RenderRequest,
    recoveries: &mut Vec<CoordinateOmission>,
) -> Result<(), ResolveSceneError> {
    recoveries.try_reserve(1)?;
    recoveries.push(CoordinateOmission {
        delta: ResourceDelta::default(),
        paint_order: (false, source_order, 0),
        source_order,
        id: id.map(try_owned).transpose()?,
        pointer: try_owned(pointer)?,
        diagnostic: value
            .map(|value| {
                coordinates::crossing(value, request).diagnostic_with_recovery(
                    request.format(),
                    id,
                    pointer,
                    LocalRecovery::Omitted,
                    semantic,
                )
            })
            .transpose()?,
    });
    Ok(())
}

pub(super) fn omit_resolved(
    element: &ResolvedElement,
    value: f64,
    request: &RenderRequest,
    recoveries: &mut Vec<CoordinateOmission>,
) -> Result<(), ResolveSceneError> {
    let (id, pointer, semantic) = match element {
        ResolvedElement::Outline(e) => (Some(e.source.id()), e.source.pointer(), true),
        ResolvedElement::Freedraw(e) => (Some(e.source.id()), e.source.pointer(), true),
        ResolvedElement::Text(e) => (Some(e.source.id()), e.source.pointer(), true),
        ResolvedElement::Frame(e) => (Some(e.source.id()), e.source.pointer(), true),
        ResolvedElement::Image(e) => (
            Some(e.source.id()),
            e.source.pointer(),
            e.link.is_some() || e.asset.sanitized_svg.is_some(),
        ),
        ResolvedElement::ImageFallback(e) => (Some(e.source.id()), e.source.pointer(), false),
        ResolvedElement::Fallback(e) => (
            e.source.id.as_deref(),
            e.source.pointer.as_str(),
            e.link.is_some() || e.label.is_some(),
        ),
    };
    omit(
        id,
        pointer,
        element.source_order(),
        Some(value),
        semantic,
        request,
        recoveries,
    )?;
    let observation = recoveries.last_mut().expect("just recorded omission");
    observation.paint_order = element.paint_order();
    observation.delta = match element {
        ResolvedElement::Outline(outline) => outline
            .generated_label
            .as_ref()
            .map_or(ResourceDelta::default(), generated_label_delta),
        ResolvedElement::Fallback(fallback) => {
            let geometry_segments = fallback.geometry_command_count()
                + fallback
                    .frame_clips
                    .iter()
                    .map(|c| c.path.operations.len() as u64)
                    .sum::<u64>();
            ResourceDelta {
                geometry_segments,
                working_bytes: 128 + geometry_segments * 64,
                total_work_units: (geometry_segments
                    + fallback.label.as_ref().map_or(0, |label| label.glyph_count))
                    * 32,
                ..ResourceDelta::default()
            }
        }
        ResolvedElement::Image(image) => {
            let geometry_segments = image
                .corner_clip
                .as_ref()
                .map_or(0, |clip| clip.path.operations.len() as u64);
            ResourceDelta {
                geometry_segments,
                total_work_units: geometry_segments * 32,
                ..ResourceDelta::default()
            }
        }
        _ => ResourceDelta::default(),
    };
    let clips = match element {
        ResolvedElement::Outline(outline) => frame_clip_delta(&outline.frame_clips),
        ResolvedElement::Freedraw(freedraw) => frame_clip_delta(&freedraw.frame_clips),
        _ => ResourceDelta::default(),
    };
    observation.delta.geometry_segments = observation
        .delta
        .geometry_segments
        .saturating_add(clips.geometry_segments);
    observation.delta.total_work_units = observation
        .delta
        .total_work_units
        .saturating_add(clips.total_work_units);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn admit_layout(
    outlines: &mut Vec<(NormalizedElement, Option<GeneratedGeometry>)>,
    fallbacks: &mut Vec<SurfaceFallback>,
    names: &mut Vec<(usize, FrameName)>,
    relationships: &ResolvedRelationshipGraph,
    rendering: EffectiveFrameRendering,
    viewport: Option<FrameViewport>,
    request: &RenderRequest,
    recoveries: &mut Vec<CoordinateOmission>,
    texts: &mut [ResolvedTextElement],
    images: &ResolvedImageInputs,
    labels: &mut PreparedLabels,
) -> Result<Option<(f64, f64, f64, f64)>, ResolveSceneError> {
    let range = CoordinateRange(request.resource_limits().max_coordinate_magnitude());
    // Each outline looks up its own text. The texts keep their order here, so
    // they are keyed once rather than scanned per outline.
    let text_positions = SourcePositions::new(texts.iter().map(|text| text.source.source_index()))?;
    loop {
        let mut rejected = Vec::new();
        rejected.try_reserve(outlines.len().saturating_add(fallbacks.len()))?;
        // Bound-text placement changed normalized positions; frame names are
        // new source-space boxes. Admit both before the bounds pass reads them.
        for (source, geometry) in outlines.iter() {
            let check = coordinates::source_derived(source, range).and_then(|()| {
                let (x1, y1, x2, y2) = bounds::element_bounds(source, geometry.as_ref());
                if x1 != f64::INFINITY {
                    range.values([x1, y1, x2, y2, x2 - x1, y2 - y1])?;
                }
                labels.check(
                    source.source.source_index(),
                    range,
                    ResolvedTransform {
                        translation_x: source.x,
                        translation_y: source.y,
                        angle_radians: source.angle,
                        center_x: source.width / 2.0,
                        center_y: source.height / 2.0,
                    },
                )?;
                if let NormalizedElementKind::Image(image) = &source.kind {
                    if let Some(asset) = images.assets.get(&source.source.source_index()) {
                        let rect = ResolvedImageSourceRect::for_image(asset, image.crop);
                        range.rect(rect.x, rect.y, rect.width, rect.height)?;
                    }
                }
                if let Some(text) = text_positions
                    .first(source.source.source_index())
                    .and_then(|position| texts.get(position))
                {
                    range.text(&text.text, text.transform)?;
                }
                if let Some((_, name)) = names
                    .iter()
                    .find(|(i, _)| *i == source.source.source_index())
                {
                    let y = source.y - FRAME_NAME_OFFSET_Y - FRAME_NAME_BOX_HEIGHT;
                    range.rect(source.x, y, name.width, FRAME_NAME_BOX_HEIGHT)?;
                    range.label(
                        &name.label,
                        ResolvedTransform {
                            translation_x: source.x,
                            translation_y: y,
                            ..coordinates::IDENTITY
                        },
                    )?;
                }
                Ok(())
            });
            if let Err(value) = check {
                rejected.push((source.source.source_index(), Some(value)));
            }
        }
        for fallback in fallbacks.iter() {
            let transform = ResolvedTransform {
                translation_x: fallback.x,
                translation_y: fallback.y,
                angle_radians: fallback.angle,
                center_x: fallback.width / 2.0,
                center_y: fallback.height / 2.0,
            };
            if let Err(value) = range
                .box_at(fallback.width, fallback.height, transform)
                .and_then(|()| labels.check(fallback.source_index, range, transform))
            {
                rejected.push((fallback.source_index, Some(value)));
            }
        }
        if rejected.is_empty() {
            let Some(bounds) = scene_bounds(
                outlines,
                fallbacks,
                names,
                relationships,
                rendering,
                viewport,
            ) else {
                return Ok(None);
            };
            if bounds.0 == f64::INFINITY {
                return Ok(Some(bounds));
            }
            for (source, geometry) in outlines.iter() {
                if geometry.is_none()
                    && matches!(
                        source.kind,
                        NormalizedElementKind::Rectangle
                            | NormalizedElementKind::Diamond
                            | NormalizedElementKind::Ellipse
                            | NormalizedElementKind::Linear(_)
                            | NormalizedElementKind::Freedraw(_)
                            | NormalizedElementKind::Embeddable { .. }
                    )
                {
                    continue;
                }
                let center = geometry
                    .as_ref()
                    .map_or((source.width / 2.0, source.height / 2.0), |g| {
                        outline::rotation_center(source, g, request.format())
                    });
                let transform = ResolvedTransform {
                    translation_x: source.x - bounds.0 + request.padding(),
                    translation_y: source.y - bounds.1 + request.padding(),
                    angle_radians: source.angle,
                    center_x: center.0,
                    center_y: center.1,
                };
                let mut check = range.box_at(source.width, source.height, transform);
                if check.is_ok() {
                    check = labels.check(source.source.source_index(), range, transform);
                }
                if check.is_ok() {
                    if let NormalizedElementKind::Image(image) = &source.kind {
                        check = range.image_projection(
                            ResolvedImageProjection::for_image(
                                source.width,
                                source.height,
                                image.crop,
                            ),
                            image.scale,
                            source.width,
                            source.height,
                            transform,
                        );
                    }
                }
                if check.is_ok() {
                    if let Some(text) = text_positions
                        .first(source.source.source_index())
                        .and_then(|position| texts.get(position))
                    {
                        check = range.text(&text.text, transform);
                    }
                }
                if check.is_ok() {
                    check = match geometry {
                        Some(GeneratedGeometry::Rough(g)) => range.drawing(&g.drawing, transform),
                        Some(GeneratedGeometry::Freedraw(g)) => range.freedraw(&g.path, transform),
                        None => Ok(()),
                    };
                }
                if check.is_ok()
                    && matches!(
                        source.kind,
                        NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
                    )
                    && rendering.enabled
                {
                    let path = if rendering.rectangular_clip {
                        resolved_frame_rect(source.width, source.height)?
                    } else {
                        resolved_frame_outline(source.width, source.height)?
                    };
                    check = range.path(&path, transform);
                }
                if check.is_ok() {
                    if let Some((_, name)) = names
                        .iter()
                        .find(|(i, _)| *i == source.source.source_index())
                    {
                        let transform = ResolvedTransform {
                            translation_x: source.x - bounds.0 + request.padding(),
                            translation_y: source.y
                                - FRAME_NAME_OFFSET_Y
                                - FRAME_NAME_BOX_HEIGHT
                                - bounds.1
                                + request.padding(),
                            center_x: name.width / 2.0,
                            center_y: FRAME_NAME_BOX_HEIGHT / 2.0,
                            ..coordinates::IDENTITY
                        };
                        check = range
                            .box_at(name.width, FRAME_NAME_BOX_HEIGHT, transform)
                            .and_then(|()| range.label(&name.label, transform));
                    }
                }
                if let Err(value) = check {
                    rejected.push((source.source.source_index(), Some(value)));
                }
            }
            for fallback in fallbacks.iter() {
                let transform = ResolvedTransform {
                    translation_x: fallback.x - bounds.0 + request.padding(),
                    translation_y: fallback.y - bounds.1 + request.padding(),
                    angle_radians: fallback.angle,
                    center_x: fallback.width / 2.0,
                    center_y: fallback.height / 2.0,
                };
                if let Err(value) = range
                    .box_at(fallback.width, fallback.height, transform)
                    .and_then(|()| labels.check(fallback.source_index, range, transform))
                {
                    rejected.push((fallback.source_index, Some(value)));
                }
            }
            if rejected.is_empty() {
                return Ok(Some(bounds));
            }
        }
        // Use repaired frame edges, never persisted IDs. Removing a clip cannot
        // expose a descendant, even when that descendant precedes its frame.
        if rendering.enabled && rendering.clip {
            loop {
                let before = rejected.len();
                for (source, _) in outlines.iter() {
                    let index = source.source.source_index();
                    if rejected.iter().any(|(i, _)| *i == index) {
                        continue;
                    }
                    if super::super::frame_target::relationship_frame_target(index, relationships)
                        .is_some_and(|parent| rejected.iter().any(|(i, _)| *i == parent))
                    {
                        rejected.push((index, None));
                    }
                }
                for fallback in fallbacks.iter() {
                    if rejected.iter().any(|(i, _)| *i == fallback.source_index) {
                        continue;
                    }
                    if fallback::fallback_parent_index(fallback, fallbacks, relationships)
                        .is_some_and(|parent| rejected.iter().any(|(i, _)| *i == parent))
                    {
                        rejected.push((fallback.source_index, None));
                    }
                }
                if before == rejected.len() {
                    break;
                }
            }
        }
        let mut index = 0;
        while index < outlines.len() {
            if let Some((_, value)) = rejected
                .iter()
                .find(|(i, _)| *i == outlines[index].0.source.source_index())
            {
                let (source, geometry) = outlines.remove(index);
                if let Some(text) = text_positions
                    .first(source.source.source_index())
                    .and_then(|position| texts.get_mut(position))
                {
                    if let Some(value) = value {
                        text.coordinate_failure = Some(*value);
                        continue;
                    }
                }
                let semantic = geometry.is_some()
                    || images
                        .assets
                        .get(&source.source.source_index())
                        .is_some_and(|asset| {
                            source.link.is_some() || asset.sanitized_svg.is_some()
                        })
                    || matches!(
                        source.kind,
                        NormalizedElementKind::Text(_)
                            | NormalizedElementKind::Frame(_)
                            | NormalizedElementKind::MagicFrame(_)
                    );
                omit(
                    Some(source.source.id()),
                    source.source.pointer(),
                    source.source_order,
                    *value,
                    semantic,
                    request,
                    recoveries,
                )?;
                let observation = recoveries.last_mut().expect("just recorded omission");
                if let Some((text, _, _)) = labels.take_embeddable(source.source.source_index()) {
                    observation.delta = generated_label_delta(&text);
                }
                observation.paint_order = (
                    matches!(source.kind, NormalizedElementKind::Embeddable { .. }),
                    source.source_order,
                    0,
                );
                if let Some(text) = text_positions
                    .first(source.source.source_index())
                    .and_then(|position| texts.get(position))
                {
                    observation.paint_order = text
                        .container_source_order
                        .map_or((false, text.source_order, 0), |order| (false, order, 1));
                }
                if let Some((_, name)) = names
                    .iter()
                    .find(|(i, _)| *i == source.source.source_index())
                {
                    let geometry_segments = name.label.path.operations.len() as u64;
                    let text_scalars = name.label.text.chars().count() as u64;
                    observation.delta = ResourceDelta {
                        geometry_segments,
                        text_scalars,
                        working_bytes: 128 + geometry_segments * 64 + text_scalars * 4,
                        total_work_units: (geometry_segments + text_scalars) * 32,
                        ..ResourceDelta::default()
                    };
                }
            } else {
                index += 1;
            }
        }
        let mut index = 0;
        while index < fallbacks.len() {
            if let Some((_, value)) = rejected
                .iter()
                .find(|(i, _)| *i == fallbacks[index].source_index)
            {
                let fallback = fallbacks.remove(index);
                let label = labels.take_fallback(fallback.source_index);
                omit(
                    fallback.element_id.as_deref(),
                    &fallback.json_pointer,
                    fallback.source_order,
                    *value,
                    true,
                    request,
                    recoveries,
                )?;
                if let Some(label) = label {
                    let geometry_segments = if fallback.width >= 2.0 && fallback.height >= 2.0 {
                        9
                    } else {
                        5
                    } + label.path.operations.len() as u64;
                    recoveries.last_mut().expect("just recorded omission").delta = ResourceDelta {
                        geometry_segments,
                        working_bytes: 128 + geometry_segments * 64,
                        total_work_units: (geometry_segments + label.glyph_count) * 32,
                        ..ResourceDelta::default()
                    };
                }
            } else {
                index += 1;
            }
        }
        names.retain(|(index, _)| !rejected.iter().any(|(i, _)| i == index));
    }
}

pub(in crate::render) fn generated_label_delta(text: &ResolvedText) -> ResourceDelta {
    let geometry_segments = text.outline_segments();
    let glyphs = text.glyph_count();
    ResourceDelta {
        geometry_segments,
        working_bytes: geometry_segments
            .saturating_mul(64)
            .saturating_add(glyphs.saturating_mul(64))
            .saturating_add(text.line_break_steps.saturating_mul(32))
            .saturating_add(text.run_count().saturating_mul(128))
            .saturating_add(text.fallback_resource_count().saturating_mul(256)),
        total_work_units: geometry_segments
            .saturating_add(glyphs)
            .saturating_add(text.scalar_count)
            .saturating_add(text.line_break_steps)
            .saturating_add(text.fallback_probes)
            .saturating_mul(32),
        ..ResourceDelta::default()
    }
}
