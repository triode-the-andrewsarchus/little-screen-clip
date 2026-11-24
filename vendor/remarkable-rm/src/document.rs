//! Top-level page representation, assembled from blocks per spec §10.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::blocks::{BlockType, iter_blocks};
use crate::crdt::CrdtId;
use crate::error::ParseError;
use crate::primitives::Reader;
use crate::scene::glyph::{GlyphRange, read_glyph};
use crate::scene::items::{ItemType, read_scene_item};
use crate::scene::line::Line;
use crate::scene::text::{RootText, read_root_text_block};
use crate::scene::tree::{Anchor, SceneTreeBlock, read_scene_tree_block, read_tree_node_block};

#[derive(Debug, Clone, Default)]
pub struct Page {
    pub layers: Vec<Layer>,
    pub text: Option<RootText>,
    /// Optional, present in firmware 3.14+ via `SceneInfoBlock` (spec §5.6).
    pub paper_size: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub node_id: CrdtId,
    pub name: String,
    pub visible: bool,
    pub lines: Vec<Line>,
    pub glyphs: Vec<GlyphRange>,
}

/// Parse one full `.rm` v6 file into a [`Page`]. Top-level entry point for
/// downstream renderers.
pub fn parse_page(bytes: &[u8]) -> Result<Page, ParseError> {
    let mut intermediate = Intermediate::default();

    for block in iter_blocks(bytes)? {
        let block = block?;
        let cur_version = block.header.current_version;
        let mut body = block.body;
        match block.header.kind() {
            Some(BlockType::SceneTree) => {
                intermediate.trees.push(read_scene_tree_block(&mut body)?);
            }
            Some(BlockType::TreeNode) => {
                let n = read_tree_node_block(&mut body)?;
                intermediate.nodes.push(NodeProps {
                    node_id: n.node_id,
                    label: n.label.value,
                    visible: n.visible.value,
                    anchor: n.anchor,
                });
            }
            Some(BlockType::SceneGroupItem) => {
                let envelope = read_scene_item(&mut body, ItemType::Group, |sub| sub.read_id(2))?;
                if envelope.deleted_length == 0
                    && let Some(node_id) = envelope.value
                {
                    intermediate.scene_items.push(SceneItemRecord {
                        parent_id: envelope.parent_id,
                        item_id: envelope.item_id,
                        left_id: envelope.left_id,
                        right_id: envelope.right_id,
                        kind: SceneItemKind::Group { node_id },
                    });
                }
            }
            Some(BlockType::SceneLineItem) => {
                let envelope = read_scene_item(&mut body, ItemType::Line, |sub| {
                    crate::scene::line::read_line(sub, cur_version)
                })?;
                if envelope.deleted_length == 0
                    && let Some(line) = envelope.value
                {
                    intermediate.scene_items.push(SceneItemRecord {
                        parent_id: envelope.parent_id,
                        item_id: envelope.item_id,
                        left_id: envelope.left_id,
                        right_id: envelope.right_id,
                        kind: SceneItemKind::Line(line),
                    });
                }
            }
            Some(BlockType::SceneGlyphItem) => {
                let envelope = read_scene_item(&mut body, ItemType::GlyphRange, read_glyph)?;
                if envelope.deleted_length == 0
                    && let Some(glyph) = envelope.value
                {
                    intermediate.scene_items.push(SceneItemRecord {
                        parent_id: envelope.parent_id,
                        item_id: envelope.item_id,
                        left_id: envelope.left_id,
                        right_id: envelope.right_id,
                        kind: SceneItemKind::Glyph(glyph),
                    });
                }
            }
            Some(BlockType::RootText) => {
                intermediate.text = Some(read_root_text_block(&mut body)?);
            }
            Some(BlockType::SceneInfo) => {
                intermediate.paper_size = read_paper_size(&mut body)?;
            }
            // MigrationInfo, AuthorIds, PageInfo,
            // SceneTextItem, SceneTombstoneItem, and unknown types are
            // consumed but not surfaced (renderer doesn't need them yet).
            // TODO: Be explicit about non-used types.
            _ => {}
        }
    }

    intermediate.assemble()
}

#[derive(Debug, Default)]
struct Intermediate {
    trees: Vec<SceneTreeBlock>,
    nodes: Vec<NodeProps>,
    scene_items: Vec<SceneItemRecord>,
    text: Option<RootText>,
    paper_size: Option<(u32, u32)>,
}

#[derive(Debug)]
struct NodeProps {
    node_id: CrdtId,
    label: String,
    visible: bool,
    anchor: Option<Anchor>,
}

#[derive(Debug)]
struct SceneItemRecord {
    parent_id: CrdtId,
    item_id: CrdtId,
    left_id: CrdtId,
    right_id: CrdtId,
    kind: SceneItemKind,
}

#[derive(Debug)]
enum SceneItemKind {
    Group { node_id: CrdtId },
    Line(Line),
    Glyph(GlyphRange),
}

impl Intermediate {
    fn assemble(self) -> Result<Page, ParseError> {
        let Self {
            trees: _trees,
            nodes,
            scene_items,
            text,
            paper_size,
        } = self;

        let layout = text
            .as_ref()
            .map(RootText::layout)
            .transpose()?
            .unwrap_or_default();
        let offsets: HashMap<_, _> = nodes
            .iter()
            .filter_map(|n| {
                n.anchor.as_ref().map(|a| {
                    (
                        n.node_id,
                        (
                            a.anchor_origin_x.value,
                            layout
                                .anchors
                                .get(&a.anchor_id.value)
                                .copied()
                                .unwrap_or(0.0),
                        ),
                    )
                })
            })
            .collect();
        let mut node_props: HashMap<CrdtId, (String, bool)> = nodes
            .into_iter()
            .map(|n| (n.node_id, (n.label, n.visible)))
            .collect();

        let mut items_by_parent: HashMap<CrdtId, Vec<SceneItemRecord>> = HashMap::new();
        for item in scene_items {
            items_by_parent
                .entry(item.parent_id)
                .or_default()
                .push(item);
        }

        let root_items = items_by_parent.remove(&CrdtId::ROOT).unwrap_or_default();
        let mut layers = Vec::new();
        let mut seen: HashSet<CrdtId> = HashSet::new();
        for item in order_scene_items(root_items) {
            let SceneItemKind::Group { node_id } = item.kind else {
                continue;
            };
            if !seen.insert(node_id) {
                continue;
            }
            let (name, visible) = node_props
                .remove(&node_id)
                .unwrap_or_else(|| (String::new(), true));
            let mut layer_lines = Vec::new();
            let mut layer_glyphs = Vec::new();
            collect_lines(
                node_id,
                &mut items_by_parent,
                &mut layer_lines,
                &mut layer_glyphs,
                &offsets,
                (0.0, 0.0),
            );
            layers.push(Layer {
                node_id,
                name,
                visible,
                lines: layer_lines,
                glyphs: layer_glyphs,
            });
        }

        Ok(Page {
            layers,
            text,
            paper_size,
        })
    }
}

fn collect_lines(
    parent_id: CrdtId,
    items_by_parent: &mut HashMap<CrdtId, Vec<SceneItemRecord>>,
    lines: &mut Vec<Line>,
    glyphs: &mut Vec<GlyphRange>,
    offsets: &HashMap<CrdtId, (f32, f32)>,
    parent_offset: (f32, f32),
) {
    let Some(items) = items_by_parent.remove(&parent_id) else {
        return;
    };

    let offset = offsets.get(&parent_id).copied().unwrap_or_default();
    let offset = (offset.0 + parent_offset.0, offset.1 + parent_offset.1);
    for item in order_scene_items(items) {
        match item.kind {
            SceneItemKind::Group { node_id } => {
                collect_lines(node_id, items_by_parent, lines, glyphs, offsets, offset);
            }
            SceneItemKind::Glyph(mut glyph) => {
                for rect in &mut glyph.rectangles {
                    rect.x += f64::from(offset.0);
                    rect.y += f64::from(offset.1);
                }
                glyphs.push(glyph);
            }
            SceneItemKind::Line(mut line) => {
                for point in &mut line.points {
                    point.x += offset.0;
                    point.y += offset.1;
                }
                lines.push(line);
            }
        }
    }
}

fn order_scene_items(items: Vec<SceneItemRecord>) -> Vec<SceneItemRecord> {
    if items.len() < 2 {
        return items;
    }

    let index_by_id: HashMap<CrdtId, usize> = items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.item_id, index))
        .collect();

    let mut edges = vec![Vec::new(); items.len()];
    let mut indegree = vec![0usize; items.len()];
    for (index, item) in items.iter().enumerate() {
        if let Some(&left) = index_by_id.get(&item.left_id) {
            add_edge(left, index, &mut edges, &mut indegree);
        }
        if let Some(&right) = index_by_id.get(&item.right_id) {
            add_edge(index, right, &mut edges, &mut indegree);
        }
    }

    let mut ready = BinaryHeap::new();
    for (index, item) in items.iter().enumerate() {
        if indegree[index] == 0 {
            ready.push(Reverse((item.item_id, index)));
        }
    }

    let mut emitted = vec![false; items.len()];
    let mut order = Vec::with_capacity(items.len());
    while let Some(Reverse((_, index))) = ready.pop() {
        if emitted[index] {
            continue;
        }
        emitted[index] = true;
        order.push(index);
        for &next in &edges[index] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push(Reverse((items[next].item_id, next)));
            }
        }
    }

    if order.len() < items.len() {
        let mut remaining: Vec<_> = items
            .iter()
            .enumerate()
            .filter(|(index, _)| !emitted[*index])
            .map(|(index, item)| (item.item_id, index))
            .collect();
        remaining.sort_unstable();
        order.extend(remaining.into_iter().map(|(_, index)| index));
    }

    let mut items = items.into_iter().map(Some).collect::<Vec<_>>();
    order
        .into_iter()
        .map(|index| items[index].take().unwrap())
        .collect()
}

fn add_edge(from: usize, to: usize, edges: &mut [Vec<usize>], indegree: &mut [usize]) {
    if from == to || edges[from].contains(&to) {
        return;
    }
    edges[from].push(to);
    indegree[to] += 1;
}

/// Extract the (optional) `paper_size` from a `SceneInfoBlock` body — spec
/// §5.6. Every field after `current_layer` is optional, detected by remaining
/// bytes in the block.
fn read_paper_size(body: &mut Reader<'_>) -> Result<Option<(u32, u32)>, ParseError> {
    let _current_layer = body.read_lww_id(1)?;
    if body.is_eof() {
        return Ok(None);
    }
    let _background_visible = body.read_lww_bool(2)?;
    if body.is_eof() {
        return Ok(None);
    }
    let _root_document_visible = body.read_lww_bool(3)?;
    if body.is_eof() {
        return Ok(None);
    }
    let mut sub = body.read_subblock(5)?;
    let width = sub.read_u32()?;
    let height = sub.read_u32()?;
    Ok(Some((width, height)))
}
