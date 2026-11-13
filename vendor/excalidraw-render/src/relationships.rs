use std::collections::TryReserveError;

use crate::normalization::NormalizedElement;
use crate::scene::{member_value, pointer_child, pointer_index, JsonValue};
use crate::{SceneDocument, SupplementalAssets};

const RELATIONSHIP_WORK_UNITS: u64 = 32;
const RELATIONSHIP_EDGE_BYTES: u64 = 64;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RelationshipKind {
    Frame,
    Group,
    Binding,
    Clip,
    Mask,
    Asset,
    Link,
}

#[derive(Clone, Debug)]
pub(crate) struct RelationshipNode {
    pub(crate) id: String,
    pub(crate) source_index: usize,
    pub(crate) source_order: u64,
    pub(crate) hit_bounds: Option<[f64; 4]>,
}

#[derive(Clone, Debug)]
pub(crate) enum RelationshipTarget {
    Element(usize),
    Group,
    Asset { authority: AssetAuthority },
    Link,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AssetAuthority {
    SceneFile,
    Supplemental,
    Missing,
}

#[derive(Clone, Debug)]
pub(crate) struct RelationshipEdge {
    pub(crate) source: usize,
    pub(crate) target: RelationshipTarget,
    pub(crate) kind: RelationshipKind,
    pub(crate) json_pointer: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedRelationshipGraph {
    pub(crate) nodes: Vec<RelationshipNode>,
    pub(crate) edges: Vec<RelationshipEdge>,
}

impl ResolvedRelationshipGraph {
    /// The node of the source element at `source_index`, if it has one.
    ///
    /// `resolve` builds the nodes in ascending source index, one per element,
    /// so a binary search finds the same node a scan from the front would,
    /// without making every per-element lookup linear in the scene.
    pub(crate) fn node_position(&self, source_index: usize) -> Option<usize> {
        self.nodes
            .binary_search_by_key(&source_index, |node| node.source_index)
            .ok()
    }

    /// The edges owned by node `node`, in graph order.
    ///
    /// `resolve` orders the edges by their owner's source order, which ascends
    /// with the owner's node index, so each node's edges are one contiguous run
    /// and the first matching edge of the run is the first in the whole graph.
    pub(crate) fn outgoing_edges(&self, node: usize) -> &[RelationshipEdge] {
        let start = self.edges.partition_point(|edge| edge.source < node);
        let end = start.saturating_add(
            self.edges
                .get(start..)
                .unwrap_or_default()
                .partition_point(|edge| edge.source == node),
        );
        self.edges.get(start..end).unwrap_or_default()
    }
}

pub(crate) struct RelationshipTraversal<'a> {
    pub(crate) element_id: &'a str,
    pub(crate) local_owner: &'a str,
    pub(crate) json_pointer: &'a str,
    pub(crate) attempted_depth: u32,
    pub(crate) retained: bool,
}

pub(crate) enum RelationshipEvent<'a> {
    SearchStep {
        element_id: &'a str,
        json_pointer: &'a str,
        attempted_depth: u32,
    },
    Traversal(RelationshipTraversal<'a>),
}

#[derive(Clone, Debug)]
pub(crate) struct RemovedCycle {
    pub(crate) element_id: String,
    pub(crate) json_pointer: String,
    pub(crate) source_order: u64,
    pub(crate) source_position: u64,
    pub(crate) source_index: usize,
    pub(crate) target_source_index: usize,
    pub(crate) channel: CycleChannel,
}

/// The source relationship's consumers, shared by its graph projections.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CycleChannel {
    FrameMembership,
    TextContainer,
    BoundElement,
    EndpointBinding,
}

#[derive(Clone)]
pub(crate) struct RelationshipOutcome {
    pub(crate) graph: ResolvedRelationshipGraph,
    pub(crate) removed_cycles: Vec<RemovedCycle>,
}

#[derive(Debug)]
pub(crate) enum RelationshipError<E> {
    Observer(E),
    MemoryUnavailable,
}

impl<E> From<TryReserveError> for RelationshipError<E> {
    fn from(_: TryReserveError) -> Self {
        Self::MemoryUnavailable
    }
}

struct Candidate {
    source: usize,
    target: CandidateTarget,
    kind: RelationshipKind,
    json_pointer: String,
    source_position: u64,
    target_source_position: u64,
    source_order: u64,
}

#[derive(Clone, Copy)]
struct CandidateOwner {
    node_index: usize,
    source_index: usize,
    source_order: u64,
}

enum CandidateTarget {
    Element(usize),
    Group,
    Asset(AssetAuthority),
    Link,
}

pub(crate) fn resolve<E>(
    scene: &SceneDocument,
    elements: &[NormalizedElement],
    supplemental_assets: &SupplementalAssets,
    mut observe: impl FnMut(RelationshipEvent<'_>) -> Result<(), E>,
) -> Result<RelationshipOutcome, RelationshipError<E>> {
    let source_count = scene.element_occurrence_count();
    let mut normalized_ids = try_filled_vec(source_count, None)?;
    // The first normalized element of each source occurrence, so a node finds
    // its element without scanning every element.
    let mut first_elements = try_filled_vec(source_count, None)?;
    for (position, element) in elements.iter().enumerate() {
        let Some(slot) = normalized_ids.get_mut(element.source.source_index()) else {
            return Err(RelationshipError::MemoryUnavailable);
        };
        *slot = Some(element.source.id());
        if let Some(first @ None) = first_elements.get_mut(element.source.source_index()) {
            *first = Some(position);
        }
    }
    let element_at = |source_index: usize| {
        first_elements
            .get(source_index)
            .copied()
            .flatten()
            .and_then(|position| elements.get(position))
    };

    let mut nodes = Vec::new();
    nodes.try_reserve_exact(source_count)?;
    for (source_index, normalized_id) in normalized_ids.iter().enumerate() {
        if scene.element_is_silent(source_index) {
            continue;
        }
        let Some(JsonValue::Object(members)) = scene.element_value(source_index) else {
            continue;
        };
        let source_id = match member_value(members, "id") {
            Some(JsonValue::String(id)) if !id.is_empty() => Some(id.as_ref()),
            _ => None,
        };
        let Some(id) = normalized_id.or(source_id) else {
            continue;
        };
        nodes.try_reserve(1)?;
        nodes.push(RelationshipNode {
            id: try_owned(id)?,
            source_index,
            source_order: scene.source_owner_order("elements", source_index),
            hit_bounds: element_at(source_index)
                .map(|element| [element.x, element.y, element.width, element.height]),
        });
    }

    // Original IDs deliberately map to the first source occurrence. Later duplicate
    // nodes remain independently owned by their normalized IDs.
    let mut aliases = Vec::<(String, usize)>::new();
    aliases.try_reserve_exact(nodes.len().saturating_mul(2))?;
    for (node_index, node) in nodes.iter().enumerate() {
        let Some(JsonValue::Object(members)) = scene.element_value(node.source_index) else {
            continue;
        };
        if let Some(JsonValue::String(original)) = member_value(members, "id") {
            if !original.is_empty() {
                aliases.push((try_owned(original)?, node_index));
            }
        }
        aliases.push((try_owned(&node.id)?, node_index));
    }
    aliases.sort_unstable_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    aliases.dedup_by(|left, right| left.0 == right.0);

    let mut scene_file_ids = Vec::new();
    scene_file_ids.try_reserve_exact(scene.file_record_count())?;
    for index in 0..scene.file_record_count() {
        if let Some(record) = scene.file_record_at(index) {
            scene_file_ids.push(record.key());
        }
    }
    scene_file_ids.sort_unstable();

    let mut candidates = Vec::new();
    for (source, node) in nodes.iter().enumerate() {
        let Some(JsonValue::Object(members)) = scene.element_value(node.source_index) else {
            continue;
        };
        let candidate_start = candidates.len();
        collect_element_candidates(
            members,
            CandidateOwner {
                node_index: source,
                source_index: node.source_index,
                source_order: node.source_order,
            },
            &aliases,
            &scene_file_ids,
            supplemental_assets,
            &mut candidates,
        )?;
        // Rendering and frame selection must use the same admitted bindings.
        // Raw source retains malformed records for embedding, not graph edges.
        if let Some(element) = element_at(node.source_index) {
            if let crate::normalization::NormalizedElementKind::Linear(linear) = &element.kind {
                let mut at = candidate_start;
                while at < candidates.len() {
                    let pointer = &candidates[at].json_pointer;
                    let omitted = (pointer.ends_with("/startBinding/elementId")
                        && linear.start_binding.is_none())
                        || (pointer.ends_with("/endBinding/elementId")
                            && linear.end_binding.is_none());
                    if omitted {
                        candidates.remove(at);
                    } else {
                        at += 1;
                    }
                }
            }
        }
    }
    candidates.sort_unstable_by(|left, right| {
        left.source_order
            .cmp(&right.source_order)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| {
                left.target_source_position
                    .cmp(&right.target_source_position)
            })
            .then_with(|| left.json_pointer.cmp(&right.json_pointer))
    });

    let mut edges = Vec::new();
    edges.try_reserve_exact(candidates.len())?;
    let mut adjacency = Vec::new();
    adjacency.try_reserve_exact(nodes.len())?;
    adjacency.resize_with(nodes.len(), Vec::new);
    let mut reverse = Vec::new();
    reverse.try_reserve_exact(nodes.len())?;
    reverse.resize_with(nodes.len(), Vec::new);
    let mut removed_cycles = Vec::new();

    for candidate in candidates {
        let Some(owner) = nodes.get(candidate.source) else {
            return Err(RelationshipError::MemoryUnavailable);
        };
        let dependency = match candidate.target {
            CandidateTarget::Element(target) => {
                let reverse = candidate.kind == RelationshipKind::Binding
                    && (candidate.json_pointer.contains("/boundElements/")
                        || candidate.json_pointer.contains("/boundElementIds/"));
                Some(if reverse {
                    (target, candidate.source)
                } else {
                    (candidate.source, target)
                })
            }
            _ => None,
        };
        let mut charge_search_step = |attempted_depth| {
            observe(RelationshipEvent::SearchStep {
                element_id: &owner.id,
                json_pointer: &candidate.json_pointer,
                attempted_depth,
            })
            .map_err(RelationshipError::Observer)
        };
        let closes_cycle = match dependency {
            Some((from, to)) => {
                from == to || path_exists(to, from, &adjacency, &mut charge_search_step)?
            }
            None => false,
        };
        let attempted_depth = dependency
            .filter(|_| !closes_cycle)
            .map(|(from, to)| {
                relationship_depth(from, to, &adjacency, &reverse, &mut charge_search_step)
            })
            .transpose()?
            .unwrap_or(0);
        let retained = !closes_cycle;
        observe(RelationshipEvent::Traversal(RelationshipTraversal {
            element_id: &owner.id,
            local_owner: &owner.id,
            json_pointer: &candidate.json_pointer,
            attempted_depth,
            retained,
        }))
        .map_err(RelationshipError::Observer)?;

        if closes_cycle {
            let CandidateTarget::Element(target) = candidate.target else {
                unreachable!("only element dependencies can close cycles");
            };
            let channel = match candidate.kind {
                RelationshipKind::Frame | RelationshipKind::Clip => CycleChannel::FrameMembership,
                RelationshipKind::Mask => CycleChannel::TextContainer,
                RelationshipKind::Binding if candidate.json_pointer.ends_with("/containerId") => {
                    CycleChannel::TextContainer
                }
                RelationshipKind::Binding
                    if candidate.json_pointer.contains("/boundElements/")
                        || candidate.json_pointer.contains("/boundElementIds/") =>
                {
                    CycleChannel::BoundElement
                }
                RelationshipKind::Binding => CycleChannel::EndpointBinding,
                _ => unreachable!("terminal relationships cannot close cycles"),
            };
            removed_cycles.try_reserve(1)?;
            removed_cycles.push(RemovedCycle {
                element_id: try_owned(&owner.id)?,
                json_pointer: candidate.json_pointer,
                source_order: owner.source_order,
                source_position: candidate.source_position,
                source_index: owner.source_index,
                target_source_index: nodes[target].source_index,
                channel,
            });
            continue;
        }
        if let Some((from, to)) = dependency {
            let Some(outgoing) = adjacency.get_mut(from) else {
                return Err(RelationshipError::MemoryUnavailable);
            };
            outgoing.try_reserve(1)?;
            outgoing.push(to);
            let Some(incoming) = reverse.get_mut(to) else {
                return Err(RelationshipError::MemoryUnavailable);
            };
            incoming.try_reserve(1)?;
            incoming.push(from);
        }
        let target = match candidate.target {
            CandidateTarget::Element(target) => RelationshipTarget::Element(target),
            CandidateTarget::Group => RelationshipTarget::Group,
            CandidateTarget::Asset(authority) => RelationshipTarget::Asset { authority },
            CandidateTarget::Link => RelationshipTarget::Link,
        };
        edges.push(RelationshipEdge {
            source: candidate.source,
            target,
            kind: candidate.kind,
            json_pointer: candidate.json_pointer,
        });
    }

    // One source relationship can project to multiple graph edges: frameId
    // supplies Frame and Clip, and containerId supplies Binding and Mask.
    // Traverse and remove each projection, but diagnose the source repair once.
    removed_cycles.sort_unstable_by(|left, right| {
        left.source_order
            .cmp(&right.source_order)
            .then_with(|| left.source_position.cmp(&right.source_position))
            .then_with(|| left.json_pointer.cmp(&right.json_pointer))
    });
    removed_cycles.dedup_by(|left, right| left.json_pointer == right.json_pointer);

    // The keyed lookups (`node_position`, `outgoing_edges`) rely on this order.
    debug_assert!(nodes
        .windows(2)
        .all(|pair| pair[0].source_index < pair[1].source_index));
    debug_assert!(edges
        .windows(2)
        .all(|pair| pair[0].source <= pair[1].source));
    Ok(RelationshipOutcome {
        graph: ResolvedRelationshipGraph { nodes, edges },
        removed_cycles,
    })
}

fn collect_element_candidates(
    members: &[crate::scene::JsonMember],
    owner: CandidateOwner,
    aliases: &[(String, usize)],
    scene_file_ids: &[&str],
    supplemental_assets: &SupplementalAssets,
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    let element_kind = match member_value(members, "type") {
        Some(JsonValue::String(kind)) => kind.as_ref(),
        _ => "",
    };
    collect_element_reference(
        members,
        "frameId",
        owner,
        RelationshipKind::Frame,
        aliases,
        candidates,
    )?;
    if let Some(JsonValue::Array(groups)) = member_value(members, "groupIds") {
        if groups
            .iter()
            .all(|value| matches!(value, JsonValue::String(_)))
        {
            for (index, value) in groups.iter().enumerate() {
                let JsonValue::String(group) = value else {
                    continue;
                };
                if group.is_empty() {
                    continue;
                }
                push_candidate(
                    members,
                    "groupIds",
                    Some(index),
                    owner,
                    CandidateTarget::Group,
                    RelationshipKind::Group,
                    u64::MAX,
                    candidates,
                )?;
            }
        }
    }
    if matches!(element_kind, "line" | "arrow" | "draw") {
        collect_binding(members, "startBinding", owner, aliases, candidates)?;
        collect_binding(members, "endBinding", owner, aliases, candidates)?;
    }
    collect_bound_elements(members, owner, aliases, candidates)?;
    if element_kind == "text" {
        collect_element_reference(
            members,
            "containerId",
            owner,
            RelationshipKind::Binding,
            aliases,
            candidates,
        )?;
    }
    collect_element_reference(
        members,
        "frameId",
        owner,
        RelationshipKind::Clip,
        aliases,
        candidates,
    )?;
    if element_kind == "text" {
        collect_element_reference(
            members,
            "containerId",
            owner,
            RelationshipKind::Mask,
            aliases,
            candidates,
        )?;
    }
    if element_kind == "image" {
        collect_terminal(
            members,
            "fileId",
            owner,
            RelationshipKind::Asset,
            TerminalAssetSources {
                scene_file_ids,
                supplemental_assets,
            },
            candidates,
        )?;
    }
    collect_terminal(
        members,
        "link",
        owner,
        RelationshipKind::Link,
        TerminalAssetSources {
            scene_file_ids,
            supplemental_assets,
        },
        candidates,
    )?;
    Ok(())
}

fn collect_element_reference(
    members: &[crate::scene::JsonMember],
    field: &str,
    owner: CandidateOwner,
    kind: RelationshipKind,
    aliases: &[(String, usize)],
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    let Some(JsonValue::String(id)) = member_value(members, field) else {
        return Ok(());
    };
    let Some(target) = resolve_alias(id, aliases) else {
        return Ok(());
    };
    push_candidate(
        members,
        field,
        None,
        owner,
        CandidateTarget::Element(target),
        kind,
        u64::try_from(target).unwrap_or(u64::MAX),
        candidates,
    )
}

fn collect_binding(
    members: &[crate::scene::JsonMember],
    field: &str,
    owner: CandidateOwner,
    aliases: &[(String, usize)],
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    let Some(JsonValue::Object(binding)) = member_value(members, field) else {
        return Ok(());
    };
    let Some(JsonValue::String(id)) = member_value(binding, "elementId") else {
        return Ok(());
    };
    let Some(target) = resolve_alias(id, aliases) else {
        return Ok(());
    };
    let mut pointer = pointer_child("", field)?;
    pointer = pointer_child(&pointer, "elementId")?;
    push_candidate_at(
        members,
        field,
        owner,
        CandidateTarget::Element(target),
        RelationshipKind::Binding,
        u64::try_from(target).unwrap_or(u64::MAX),
        pointer,
        binding
            .iter()
            .position(|member| &*member.key == "elementId"),
        candidates,
    )
}

fn collect_bound_elements(
    members: &[crate::scene::JsonMember],
    owner: CandidateOwner,
    aliases: &[(String, usize)],
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    if let Some(JsonValue::Array(records)) = member_value(members, "boundElements") {
        if !records
            .iter()
            .all(|record| matches!(record, JsonValue::Object(_)))
        {
            return Ok(());
        }
        for (index, record) in records.iter().enumerate() {
            let JsonValue::Object(record) = record else {
                continue;
            };
            let Some(JsonValue::String(id)) = member_value(record, "id") else {
                continue;
            };
            if id.is_empty()
                || !matches!(member_value(record,"type"),Some(JsonValue::String(kind)) if matches!(kind.as_ref(),"arrow"|"text"))
            {
                continue;
            }
            push_bound_element_candidate(
                members,
                "boundElements",
                index,
                id,
                owner,
                aliases,
                candidates,
            )?;
        }
    } else if member_value(members, "boundElements").is_none() {
        let Some(JsonValue::Array(ids)) = member_value(members, "boundElementIds") else {
            return Ok(());
        };
        if !ids.iter().all(|id| matches!(id, JsonValue::String(_))) {
            return Ok(());
        }
        for (index, id) in ids.iter().enumerate() {
            let JsonValue::String(id) = id else {
                continue;
            };
            push_bound_element_candidate(
                members,
                "boundElementIds",
                index,
                id,
                owner,
                aliases,
                candidates,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_bound_element_candidate(
    members: &[crate::scene::JsonMember],
    field: &str,
    index: usize,
    id: &str,
    owner: CandidateOwner,
    aliases: &[(String, usize)],
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    let Some(target) = resolve_alias(id, aliases) else {
        return Ok(());
    };
    let mut pointer = pointer_index(&pointer_child("", field)?, index)?;
    if field == "boundElements" {
        pointer = pointer_child(&pointer, "id")?;
    }
    push_candidate_at(
        members,
        field,
        owner,
        CandidateTarget::Element(target),
        RelationshipKind::Binding,
        u64::try_from(target).unwrap_or(u64::MAX),
        pointer,
        Some(index),
        candidates,
    )
}

#[derive(Clone, Copy)]
struct TerminalAssetSources<'a> {
    scene_file_ids: &'a [&'a str],
    supplemental_assets: &'a SupplementalAssets,
}

fn collect_terminal(
    members: &[crate::scene::JsonMember],
    field: &str,
    owner: CandidateOwner,
    kind: RelationshipKind,
    asset_sources: TerminalAssetSources<'_>,
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    let Some(JsonValue::String(value)) = member_value(members, field) else {
        return Ok(());
    };
    if value.is_empty() {
        return Ok(());
    }
    let target = match kind {
        RelationshipKind::Asset => {
            let authority = if asset_sources
                .scene_file_ids
                .binary_search(&value.as_ref())
                .is_ok()
            {
                AssetAuthority::SceneFile
            } else if asset_sources.supplemental_assets.get(value).is_some() {
                AssetAuthority::Supplemental
            } else {
                AssetAuthority::Missing
            };
            CandidateTarget::Asset(authority)
        }
        RelationshipKind::Link if crate::scene_surface::safe_link(value) => CandidateTarget::Link,
        RelationshipKind::Link => return Ok(()),
        _ => unreachable!("terminal relationship kind is asset or link"),
    };
    push_candidate(
        members,
        field,
        None,
        owner,
        target,
        kind,
        u64::MAX,
        candidates,
    )
}

#[allow(clippy::too_many_arguments)]
fn push_candidate(
    members: &[crate::scene::JsonMember],
    field: &str,
    nested_index: Option<usize>,
    owner: CandidateOwner,
    target: CandidateTarget,
    kind: RelationshipKind,
    target_source_position: u64,
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    let mut pointer = pointer_child("", field)?;
    if let Some(index) = nested_index {
        pointer = pointer_index(&pointer, index)?;
    }
    push_candidate_at(
        members,
        field,
        owner,
        target,
        kind,
        target_source_position,
        pointer,
        nested_index,
        candidates,
    )
}

#[allow(clippy::too_many_arguments)]
fn push_candidate_at(
    members: &[crate::scene::JsonMember],
    field: &str,
    owner: CandidateOwner,
    target: CandidateTarget,
    kind: RelationshipKind,
    target_source_position: u64,
    relative_pointer: String,
    nested_position: Option<usize>,
    candidates: &mut Vec<Candidate>,
) -> Result<(), TryReserveError> {
    let element_pointer = pointer_index("/elements", owner.source_index)?;
    let json_pointer = format_pointer(&element_pointer, &relative_pointer)?;
    let member = members
        .iter()
        .position(|member| &*member.key == field)
        .unwrap_or(usize::MAX);
    candidates.try_reserve(1)?;
    let source_position = u64::try_from(member)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(u32::MAX) + 1)
        .saturating_add(
            nested_position
                .and_then(|position| u64::try_from(position).ok())
                .map_or(0, |position| position.saturating_add(1)),
        );
    candidates.push(Candidate {
        source: owner.node_index,
        target,
        kind,
        json_pointer,
        source_position,
        target_source_position,
        source_order: owner.source_order,
    });
    Ok(())
}

fn resolve_alias(id: &str, aliases: &[(String, usize)]) -> Option<usize> {
    aliases
        .binary_search_by(|(alias, _)| alias.as_str().cmp(id))
        .ok()
        .and_then(|index| aliases.get(index).map(|(_, target)| *target))
}

fn path_exists<E>(
    start: usize,
    goal: usize,
    adjacency: &[Vec<usize>],
    charge: &mut impl FnMut(u32) -> Result<(), RelationshipError<E>>,
) -> Result<bool, RelationshipError<E>> {
    let mut stack = Vec::new();
    stack.try_reserve_exact(adjacency.len())?;
    stack.push((start, 0_u32));
    let mut seen = try_filled_vec(adjacency.len(), false)?;
    while let Some((node, depth)) = stack.pop() {
        if node == goal {
            return Ok(true);
        }
        let Some(was_seen) = seen.get_mut(node) else {
            continue;
        };
        if *was_seen {
            continue;
        }
        *was_seen = true;
        let Some(outgoing) = adjacency.get(node) else {
            continue;
        };
        stack.try_reserve(outgoing.len())?;
        for &next in outgoing {
            let next_depth = depth.saturating_add(1);
            charge(next_depth)?;
            stack.push((next, next_depth));
        }
    }
    Ok(false)
}

fn relationship_depth<E>(
    source: usize,
    target: usize,
    adjacency: &[Vec<usize>],
    reverse: &[Vec<usize>],
    charge: &mut impl FnMut(u32) -> Result<(), RelationshipError<E>>,
) -> Result<u32, RelationshipError<E>> {
    Ok(longest_distance(source, reverse, charge)?
        .saturating_add(1)
        .saturating_add(longest_distance(target, adjacency, charge)?))
}

fn longest_distance<E>(
    start: usize,
    adjacency: &[Vec<usize>],
    charge: &mut impl FnMut(u32) -> Result<(), RelationshipError<E>>,
) -> Result<u32, RelationshipError<E>> {
    let mut stack = Vec::new();
    stack.try_reserve_exact(adjacency.len())?;
    stack.push((start, 0_u32));
    let mut best = try_filled_vec(adjacency.len(), 0_u32)?;
    let mut maximum = 0;
    while let Some((node, depth)) = stack.pop() {
        let Some(best_depth) = best.get_mut(node) else {
            continue;
        };
        if depth < *best_depth {
            continue;
        }
        *best_depth = depth;
        maximum = maximum.max(depth);
        let Some(outgoing) = adjacency.get(node) else {
            continue;
        };
        for &next in outgoing {
            let next_depth = depth.saturating_add(1);
            charge(next_depth)?;
            if best.get(next).is_some_and(|best| next_depth > *best) {
                stack.try_reserve(1)?;
                stack.push((next, next_depth));
            }
        }
    }
    Ok(maximum)
}

fn try_filled_vec<T: Clone>(length: usize, value: T) -> Result<Vec<T>, TryReserveError> {
    let mut values = Vec::new();
    values.try_reserve_exact(length)?;
    values.resize(length, value);
    Ok(values)
}

fn format_pointer(parent: &str, relative: &str) -> Result<String, TryReserveError> {
    let mut pointer = String::new();
    pointer.try_reserve_exact(parent.len().saturating_add(relative.len()))?;
    pointer.push_str(parent);
    pointer.push_str(relative);
    Ok(pointer)
}

fn try_owned(value: &str) -> Result<String, TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}

pub(crate) const fn traversal_delta(retained: bool) -> (u64, u64) {
    (
        RELATIONSHIP_WORK_UNITS,
        if retained { RELATIONSHIP_EDGE_BYTES } else { 0 },
    )
}

pub(crate) const fn edge_working_bytes() -> u64 {
    RELATIONSHIP_EDGE_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompatibilityRevision, OutputFormat};

    #[test]
    fn duplicate_id_references_target_the_first_source_occurrence() {
        let scene = SceneDocument::parse(
            br##"{"type":"excalidraw","version":2,"elements":[{"id":"same","type":"rectangle","seed":1,"updated":1},{"id":"same","type":"rectangle","seed":1,"updated":1},{"id":"source","type":"rectangle","frameId":"same","seed":1,"updated":1}],"appState":{},"files":{}}"##,
        )
        .expect("relationship vector is admitted");
        let normalized = crate::normalization::normalize_scene(
            &scene,
            CompatibilityRevision::REVISION_1,
            OutputFormat::Svg,
            |_| Ok::<(), ()>(()),
        )
        .expect("normalization succeeds")
        .expect("revision one is supported");

        let outcome = resolve(
            &scene,
            &normalized.elements,
            &SupplementalAssets::default(),
            |_| Ok::<(), ()>(()),
        )
        .expect("relationships resolve");
        let frame = outcome
            .graph
            .edges
            .iter()
            .find(|edge| edge.kind == RelationshipKind::Frame)
            .expect("frame relationship is retained");

        assert_eq!(outcome.graph.nodes[frame.source].id, "source");
        let RelationshipTarget::Element(target) = frame.target else {
            panic!("frame target is an element");
        };
        assert_eq!(target, 0);
        assert_eq!(outcome.graph.nodes[target].source_index, 0);
        assert_ne!(outcome.graph.nodes[1].id, "same");
    }

    #[test]
    fn relationship_kinds_keep_the_frozen_canonical_order() {
        assert!(RelationshipKind::Frame < RelationshipKind::Group);
        assert!(RelationshipKind::Group < RelationshipKind::Binding);
        assert!(RelationshipKind::Binding < RelationshipKind::Clip);
        assert!(RelationshipKind::Clip < RelationshipKind::Mask);
        assert!(RelationshipKind::Mask < RelationshipKind::Asset);
        assert!(RelationshipKind::Asset < RelationshipKind::Link);
    }

    #[test]
    fn relationship_fields_follow_kind_policy_and_asset_authority() {
        let scene = SceneDocument::parse(
            br##"{"type":"excalidraw","version":2,"elements":[{"id":"shape","type":"rectangle","containerId":"ignored","seed":1,"updated":1},{"id":"ignored","type":"rectangle","seed":1,"updated":1},{"id":"image","type":"image","fileId":"supplement","status":"saved","seed":1,"updated":1}],"appState":{},"files":{}}"##,
        )
        .expect("relationship policy vector is admitted");
        let normalized = crate::normalization::normalize_scene(
            &scene,
            CompatibilityRevision::REVISION_1,
            OutputFormat::Svg,
            |_| Ok::<(), ()>(()),
        )
        .expect("normalization succeeds")
        .expect("revision one is supported");
        let supplemental = SupplementalAssets::builder()
            .asset("supplement", vec![1])
            .build()
            .expect("supplement is valid");
        let outcome = resolve(&scene, &normalized.elements, &supplemental, |_| {
            Ok::<(), ()>(())
        })
        .expect("relationships resolve");

        assert!(!outcome.graph.edges.iter().any(|edge| {
            edge.source == 0
                && matches!(
                    edge.kind,
                    RelationshipKind::Binding | RelationshipKind::Mask
                )
        }));
        assert!(outcome.graph.edges.iter().any(|edge| {
            matches!(
                edge.target,
                RelationshipTarget::Asset {
                    authority: AssetAuthority::Supplemental,
                    ..
                }
            )
        }));
    }
}
