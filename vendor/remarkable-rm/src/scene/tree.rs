//! `SceneTreeBlock` (0x01) and `TreeNodeBlock` (0x02) — spec §5.2 and §5.3.

use crate::crdt::{CrdtId, LwwValue};
use crate::error::ParseError;
use crate::primitives::Reader;

#[derive(Debug, Clone)]
pub struct SceneTreeBlock {
    pub tree_id: CrdtId,
    pub node_id: CrdtId,
    pub is_update: bool,
    pub parent_id: CrdtId,
}

#[derive(Debug, Clone)]
pub struct TreeNodeBlock {
    pub node_id: CrdtId,
    pub label: LwwValue<String>,
    pub visible: LwwValue<bool>,
    pub anchor: Option<Anchor>,
}

/// Group anchor — spec §5.3 and §10.2. Only the newer (LWW-wrapped) layout
/// at indices 7-10 is implemented; the older 4-6 layout (mentioned in the
/// spec's TODO note) is not exercised by any of our fixtures.
#[derive(Debug, Clone)]
pub struct Anchor {
    pub anchor_id: LwwValue<CrdtId>,
    pub anchor_type: LwwValue<u8>,
    pub anchor_threshold: LwwValue<f32>,
    pub anchor_origin_x: LwwValue<f32>,
}

pub fn read_scene_tree_block(body: &mut Reader<'_>) -> Result<SceneTreeBlock, ParseError> {
    let tree_id = body.read_id(1)?;
    let node_id = body.read_id(2)?;
    let is_update = body.read_bool_field(3)?;
    let mut parent_sub = body.read_subblock(4)?;
    let parent_id = parent_sub.read_id(1)?;
    Ok(SceneTreeBlock {
        tree_id,
        node_id,
        is_update,
        parent_id,
    })
}

pub fn read_tree_node_block(body: &mut Reader<'_>) -> Result<TreeNodeBlock, ParseError> {
    let node_id = body.read_id(1)?;
    let label = body.read_lww_string(2)?;
    let visible = body.read_lww_bool(3)?;

    let anchor = if body.is_eof() {
        None
    } else {
        Some(Anchor {
            anchor_id: body.read_lww_id(7)?,
            anchor_type: body.read_lww_byte(8)?,
            anchor_threshold: body.read_lww_float(9)?,
            anchor_origin_x: body.read_lww_float(10)?,
        })
    };

    Ok(TreeNodeBlock {
        node_id,
        label,
        visible,
        anchor,
    })
}
