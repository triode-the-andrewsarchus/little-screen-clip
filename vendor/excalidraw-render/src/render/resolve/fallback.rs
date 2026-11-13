//! Compatibility-placeholder resolution: the placeholder frames whose clips
//! surviving placeholder children resolve against, and one `ResolvedFallback`
//! per placeholder the scene surface paints, its label shaped in the fallback
//! font.

use crate::scene_surface::SurfaceFallback;

use super::super::frame_target::EffectiveFrameRendering;
use super::super::text::ResolvedLabel;
use super::super::try_owned;
use super::{FallbackSource, ResolveContext, ResolveSceneError, ResolvedClip, ResolvedFallback};

/// A frame that survives only as a compatibility placeholder, as the clip its
/// placeholder children resolve against.
pub(super) struct FallbackFrameClipSource {
    id: String,
    source_index: usize,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    angle: f64,
}

/// Mirrors the two clip-authority branches in `resolve_fallback`: repaired
/// graph edges for graph members, otherwise the placeholder frame lookup.
pub(super) fn fallback_parent_index(
    fallback: &SurfaceFallback,
    fallbacks: &[SurfaceFallback],
    relationships: &crate::relationships::ResolvedRelationshipGraph,
) -> Option<usize> {
    if let Some(node) = fallback
        .element_id
        .as_deref()
        .and_then(|id| relationships.nodes.iter().find(|node| node.id == id))
    {
        return super::super::frame_target::relationship_frame_target(
            node.source_index,
            relationships,
        );
    }
    fallback
        .frame_id
        .as_deref()
        .and_then(|id| {
            fallbacks
                .iter()
                .find(|parent| parent.is_frame && parent.element_id.as_deref() == Some(id))
        })
        .map(|parent| parent.source_index)
}

/// The frames that survive only as compatibility placeholders, as the clips
/// their placeholder children resolve against when frame clipping is on; empty
/// otherwise.
pub(super) fn fallback_frame_clip_sources(
    fallbacks: &[SurfaceFallback],
    frame_rendering: EffectiveFrameRendering,
) -> Result<Vec<FallbackFrameClipSource>, ResolveSceneError> {
    let mut fallback_frame_clips = Vec::new();
    if frame_rendering.enabled && frame_rendering.clip {
        fallback_frame_clips.try_reserve(fallbacks.len())?;
        for fallback in fallbacks {
            let Some(id) = fallback.element_id.as_deref().filter(|_| fallback.is_frame) else {
                continue;
            };
            fallback_frame_clips.push(FallbackFrameClipSource {
                id: try_owned(id)?,
                source_index: fallback.source_index,
                x: fallback.x,
                y: fallback.y,
                width: fallback.width,
                height: fallback.height,
                angle: fallback.angle,
            });
        }
    }
    Ok(fallback_frame_clips)
}

/// One compatibility placeholder the scene surface paints: its label shaped in
/// the fallback font, clipped and faded by the frame that owns its source
/// element, or clipped by the placeholder frame its `frameId` names when the
/// element itself did not survive into the relationship graph.
pub(super) fn resolve_fallback(
    fallback: SurfaceFallback,
    context: &ResolveContext<'_>,
    label: Option<ResolvedLabel>,
    fallback_frame_clips: &[FallbackFrameClipSource],
) -> Result<ResolvedFallback, ResolveSceneError> {
    let source_index = fallback.element_id.as_deref().and_then(|id| {
        context
            .relationships
            .nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| node.source_index)
    });
    let frame_clips = if let Some(source_index) = source_index {
        context.frame_clips(source_index)?
    } else {
        let frame = fallback.frame_id.as_deref().and_then(|frame_id| {
            fallback_frame_clips
                .iter()
                .find(|frame| frame.id == frame_id)
        });
        let mut clips = Vec::new();
        if let Some(frame) = frame {
            clips.try_reserve_exact(1)?;
            clips.push(ResolvedClip::for_frame(
                frame.source_index,
                frame.width,
                frame.height,
                true,
                context.transform(
                    frame.x,
                    frame.y,
                    frame.angle,
                    frame.width / 2.0,
                    frame.height / 2.0,
                ),
            )?);
        }
        clips
    };
    let frame_opacity =
        source_index.map_or(1.0, |source_index| context.frame_opacity(source_index));
    let transform = context.transform(
        fallback.x,
        fallback.y,
        fallback.angle,
        fallback.width / 2.0,
        fallback.height / 2.0,
    );
    Ok(ResolvedFallback {
        source_order: fallback.source_order,
        source: FallbackSource {
            id: fallback.element_id,
            pointer: fallback.json_pointer,
        },
        label,
        link: fallback.link,
        frame_clips,
        width: fallback.width,
        height: fallback.height,
        transform,
        opacity: fallback.opacity / 100.0 * frame_opacity,
        render: true,
    })
}
