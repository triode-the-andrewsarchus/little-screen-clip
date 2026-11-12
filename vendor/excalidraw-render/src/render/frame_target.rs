use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::portable_math::sin_cos;
use crate::relationships::{RelationshipKind, RelationshipTarget, ResolvedRelationshipGraph};
use crate::scene_surface::SurfaceFallback;
use crate::FrameKind;

use super::geometry::{include_transformed_bounds, source_layout_bounds};
use super::resolve::{
    relationship_element_target, ResolveSceneError, ResolvedClip, ResolvedTransform,
    SemanticFrameSource,
};
use super::try_owned;

#[derive(Clone, Copy, Debug)]
pub(super) struct FrameTarget {
    pub(super) source_index: usize,
    pub(super) ordinal: u32,
    pub(super) kind: FrameKind,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FrameViewport {
    pub(super) min_x: f64,
    pub(super) min_y: f64,
    pub(super) max_x: f64,
    pub(super) max_y: f64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct EffectiveFrameRendering {
    pub(super) enabled: bool,
    pub(super) clip: bool,
    pub(super) name: bool,
    pub(super) outline: bool,
    pub(super) rectangular_clip: bool,
    pub(super) emit_frame_elements: bool,
}

fn frame_target_viewport(
    elements: &[NormalizedElement],
    target: FrameTarget,
) -> Option<FrameViewport> {
    let frame = elements
        .iter()
        .find(|element| element.source.source_index() == target.source_index)?;
    if !matches!(
        frame.kind,
        NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
    ) {
        return None;
    }
    let (min_x, min_y, max_x, max_y) = normalized_element_aabb(frame);
    [min_x, min_y, max_x, max_y]
        .iter()
        .all(|value| value.is_finite())
        .then_some(FrameViewport {
            min_x,
            min_y,
            max_x,
            max_y,
        })
}

pub(super) fn frame_participating_sources(
    elements: &[NormalizedElement],
    relationships: &ResolvedRelationshipGraph,
    target: FrameTarget,
    viewport: FrameViewport,
) -> Result<Vec<usize>, std::collections::TryReserveError> {
    let mut selected = Vec::new();
    selected.try_reserve_exact(elements.len())?;
    for element in elements {
        let (min_x, min_y, max_x, max_y) = normalized_element_aabb(element);
        if min_x <= viewport.max_x
            && max_x >= viewport.min_x
            && min_y <= viewport.max_y
            && max_y >= viewport.min_y
        {
            selected.push(element.source.source_index());
        }
    }

    let geometric = selected.clone();
    for source_index in geometric {
        let Some(source_node) = relationships.node_position(source_index) else {
            continue;
        };
        for edge in relationships.outgoing_edges(source_node) {
            if edge.kind != RelationshipKind::Binding {
                continue;
            }
            let RelationshipTarget::Element(target_node) = edge.target else {
                continue;
            };
            if let Some(node) = relationships.nodes.get(target_node) {
                selected.push(node.source_index);
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    selected.retain(|source_index| {
        *source_index == target.source_index
            || relationship_frame_target(*source_index, relationships)
                .is_none_or(|parent| parent == target.source_index)
    });
    Ok(selected)
}

pub(super) fn normalized_element_aabb(source: &NormalizedElement) -> (f64, f64, f64, f64) {
    let (local_min_x, local_min_y, local_max_x, local_max_y) = source_layout_bounds(source);
    let center_x = source.x + (local_min_x + local_max_x) / 2.0;
    let center_y = source.y + (local_min_y + local_max_y) / 2.0;
    let (sin, cos) = sin_cos(source.angle);
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (x, y) in [
        (source.x + local_min_x, source.y + local_min_y),
        (source.x + local_max_x, source.y + local_min_y),
        (source.x + local_max_x, source.y + local_max_y),
        (source.x + local_min_x, source.y + local_max_y),
    ] {
        let delta_x = x - center_x;
        let delta_y = y - center_y;
        let rotated_x = center_x + delta_x * cos - delta_y * sin;
        let rotated_y = center_y + delta_x * sin + delta_y * cos;
        min_x = min_x.min(rotated_x);
        min_y = min_y.min(rotated_y);
        max_x = max_x.max(rotated_x);
        max_y = max_y.max(rotated_y);
    }
    (min_x, min_y, max_x, max_y)
}

fn fallback_participates_in_frame(
    fallback: &SurfaceFallback,
    target_id: &str,
    viewport: FrameViewport,
) -> bool {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    include_transformed_bounds(
        fallback.x,
        fallback.y,
        fallback.width,
        fallback.height,
        fallback.angle,
        &mut min_x,
        &mut min_y,
        &mut max_x,
        &mut max_y,
    );
    min_x <= viewport.max_x
        && max_x >= viewport.min_x
        && min_y <= viewport.max_y
        && max_y >= viewport.min_y
        && fallback
            .frame_id
            .as_deref()
            .is_none_or(|frame_id| frame_id == target_id)
}

pub(super) fn relationship_frame_target(
    source_index: usize,
    relationships: &ResolvedRelationshipGraph,
) -> Option<usize> {
    relationship_element_target(source_index, RelationshipKind::Frame, relationships)
}

pub(super) fn resolve_frame_clips(
    source_index: usize,
    frames: &[SemanticFrameSource],
    relationships: &ResolvedRelationshipGraph,
    rendering: EffectiveFrameRendering,
    min_x: f64,
    min_y: f64,
    padding: f64,
) -> Result<Vec<ResolvedClip>, ResolveSceneError> {
    let mut clips = Vec::new();
    if !rendering.enabled || !rendering.clip {
        return Ok(clips);
    }
    let Some(parent_index) = relationship_frame_target(source_index, relationships) else {
        return Ok(clips);
    };
    let Some(frame) = frames
        .iter()
        .find(|frame| frame.source_index == parent_index)
    else {
        return Ok(clips);
    };
    clips.try_reserve_exact(1)?;
    clips.push(ResolvedClip::for_frame(
        frame.source_index,
        frame.width,
        frame.height,
        !rendering.rectangular_clip,
        ResolvedTransform {
            translation_x: frame.x - min_x + padding,
            translation_y: frame.y - min_y + padding,
            angle_radians: frame.angle,
            center_x: frame.width / 2.0,
            center_y: frame.height / 2.0,
        },
    )?);
    Ok(clips)
}

pub(super) fn resolve_frame_opacity(
    source_index: usize,
    frames: &[SemanticFrameSource],
    relationships: &ResolvedRelationshipGraph,
    _rendering: EffectiveFrameRendering,
) -> f64 {
    relationship_frame_target(source_index, relationships)
        .and_then(|parent_index| {
            frames
                .iter()
                .find(|frame| frame.source_index == parent_index)
        })
        .map_or(1.0, |frame| frame.opacity)
}

/// The frame-scoped view of a normalized scene: which source records take part
/// in one frame target, and which file records the survivors still reference.
///
/// Both the batch path (which clones a shared normalization per target) and the
/// standalone path (which filters its own normalization in place) select the
/// same records; only their allocation and failure strategies differ, so the
/// decision lives here and the application stays with each caller.
#[derive(Debug)]
pub(super) struct FrameSelection {
    pub(super) viewport: FrameViewport,
    pub(super) participating: Vec<usize>,
    target_id: String,
}

impl FrameSelection {
    /// Returns `None` when the target frame is absent from the normalized
    /// elements. That is a contradiction for a discovered frame, but what it
    /// means to the caller differs: the standalone path fails the render, while
    /// the batch path falls back to the unfiltered scene.
    pub(super) fn compute(
        elements: &[NormalizedElement],
        relationships: &ResolvedRelationshipGraph,
        target: FrameTarget,
    ) -> Result<Option<Self>, std::collections::TryReserveError> {
        let Some(viewport) = frame_target_viewport(elements, target) else {
            return Ok(None);
        };
        let participating = frame_participating_sources(elements, relationships, target, viewport)?;
        let target_id = elements
            .iter()
            .find(|element| element.source.source_index() == target.source_index)
            .map_or(Ok(String::new()), |element| try_owned(element.source.id()))?;
        Ok(Some(Self {
            viewport,
            participating,
            target_id,
        }))
    }

    pub(super) fn retains_element(&self, element: &NormalizedElement) -> bool {
        self.participating
            .binary_search(&element.source.source_index())
            .is_ok()
    }

    pub(super) fn retains_fallback(&self, fallback: &SurfaceFallback) -> bool {
        self.participating
            .binary_search(&fallback.source_index)
            .is_ok()
            || fallback_participates_in_frame(fallback, &self.target_id, self.viewport)
    }
}

/// File-record keys referenced by the retained elements, sorted and
/// deduplicated so both the file retention and the diagnostic scope can use a
/// binary search.
pub(super) fn selected_file_ids(elements: &[NormalizedElement]) -> Vec<String> {
    let mut selected = elements
        .iter()
        .filter_map(|element| match &element.kind {
            NormalizedElementKind::Image(image) => image.file_id.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();
    selected.sort_unstable();
    selected.dedup();
    selected
}
