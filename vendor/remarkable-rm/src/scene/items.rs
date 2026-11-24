//! Common CRDT-sequence-item envelope shared by block types 0x03-0x06 and
//! 0x08 — spec §6.1.

use crate::crdt::CrdtId;
use crate::error::ParseError;
use crate::primitives::Reader;

#[derive(Debug, Clone)]
pub struct ItemEnvelope<V> {
    pub parent_id: CrdtId,
    pub item_id: CrdtId,
    pub left_id: CrdtId,
    pub right_id: CrdtId,
    pub deleted_length: u32,
    pub value: Option<V>,
}

/// Item-type discriminator inside the value sub-block — spec §6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ItemType {
    GlyphRange = 0x01,
    Group = 0x02,
    Line = 0x03,
    Text = 0x05,
}

/// Read a CRDT-sequence-item envelope. The value sub-block (tag 6) is
/// optional — absent for tombstones and text-item references. When present,
/// its first byte is the item-type discriminator, which must match
/// `expected_item_type`; the remainder is handed to `parse_value`.
pub fn read_scene_item<'a, V>(
    body: &mut Reader<'a>,
    expected_item_type: ItemType,
    parse_value: impl FnOnce(&mut Reader<'a>) -> Result<V, ParseError>,
) -> Result<ItemEnvelope<V>, ParseError> {
    let parent_id = body.read_id(1)?;
    let item_id = body.read_id(2)?;
    let left_id = body.read_id(3)?;
    let right_id = body.read_id(4)?;
    let deleted_length = body.read_int(5)?;

    let value = if body.is_eof() {
        None
    } else {
        let mut sub = body.read_subblock(6)?;
        let item_type = sub.read_u8()?;
        if item_type != expected_item_type as u8 {
            return Err(ParseError::InvalidBlock("scene item type mismatch"));
        }
        Some(parse_value(&mut sub)?)
    };

    Ok(ItemEnvelope {
        parent_id,
        item_id,
        left_id,
        right_id,
        deleted_length,
        value,
    })
}
