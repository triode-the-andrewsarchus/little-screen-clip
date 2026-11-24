//! CRDT identifiers and last-writer-wins values — spec §2 (`CrdtId`) and §9.

use crate::error::ParseError;
use crate::primitives::Reader;

/// CRDT identifier, doubling as a logical timestamp.
///
/// `author` is the per-file author index; `seq` is that author's sequence
/// number. The pair `(0, 0)` is the start/end sentinel; `(0, 1)` is the
/// scene-tree root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CrdtId {
    pub author: u8,
    pub seq: u64,
}

impl CrdtId {
    pub const SENTINEL: Self = Self { author: 0, seq: 0 };
    pub const ROOT: Self = Self { author: 0, seq: 1 };
}

/// Last-writer-wins wrapper. On conflict, the value with the highest
/// `timestamp` wins (spec §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LwwValue<T> {
    pub timestamp: CrdtId,
    pub value: T,
}

impl Reader<'_> {
    /// Read a raw `CrdtId` (`uint8 + varuint`) — spec §2. No tag prefix.
    pub fn read_crdt_id(&mut self) -> Result<CrdtId, ParseError> {
        let author = self.read_u8()?;
        let seq = self.read_varuint()?;
        Ok(CrdtId { author, seq })
    }

    pub fn read_lww_id(&mut self, index: u32) -> Result<LwwValue<CrdtId>, ParseError> {
        let mut sub = self.read_subblock(index)?;
        let timestamp = sub.read_id(1)?;
        let value = sub.read_id(2)?;
        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_bool(&mut self, index: u32) -> Result<LwwValue<bool>, ParseError> {
        let mut sub = self.read_subblock(index)?;
        let timestamp = sub.read_id(1)?;
        let value = sub.read_bool_field(2)?;
        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_byte(&mut self, index: u32) -> Result<LwwValue<u8>, ParseError> {
        let mut sub = self.read_subblock(index)?;
        let timestamp = sub.read_id(1)?;
        let value = sub.read_byte(2)?;
        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_float(&mut self, index: u32) -> Result<LwwValue<f32>, ParseError> {
        let mut sub = self.read_subblock(index)?;
        let timestamp = sub.read_id(1)?;
        let value = sub.read_float(2)?;
        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_string(&mut self, index: u32) -> Result<LwwValue<String>, ParseError> {
        let mut sub = self.read_subblock(index)?;
        let timestamp = sub.read_id(1)?;
        let value = sub.read_string_subblock(2)?.to_owned();
        Ok(LwwValue { timestamp, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crdt_id_basic() {
        // (1, 1)
        let mut r = Reader::new(&[0x01, 0x01]);
        assert_eq!(r.read_crdt_id().unwrap(), CrdtId { author: 1, seq: 1 });
    }

    #[test]
    fn crdt_id_sentinel() {
        let mut r = Reader::new(&[0x00, 0x00]);
        assert_eq!(r.read_crdt_id().unwrap(), CrdtId::SENTINEL);
    }

    #[test]
    fn crdt_id_multibyte_seq() {
        // author=2, seq=300 (0xAC 0x02)
        let mut r = Reader::new(&[0x02, 0xAC, 0x02]);
        assert_eq!(
            r.read_crdt_id().unwrap(),
            CrdtId {
                author: 2,
                seq: 300
            }
        );
    }

    /// Build the bytes for: tag(idx, Length4) + length(LE u32) + content.
    fn subblock_with(idx: u32, content: &[u8]) -> Vec<u8> {
        let tag = (u64::from(idx) << 4) | 0x0C;
        assert!(tag < 0x80, "test only supports single-byte tag varuints");
        #[allow(clippy::cast_possible_truncation)]
        let tag_byte = tag as u8;
        let len = u32::try_from(content.len()).unwrap();
        let mut out = vec![tag_byte];
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(content);
        out
    }

    #[test]
    fn lww_bool_round_trip() {
        // outer subblock(3) -> [ tag(1,Id)+id, tag(2,Byte1)+0x01 ]
        let mut content = vec![0x1F, 0x05, 0x07]; // id timestamp = (5, 7)
        content.extend_from_slice(&[0x21, 0x01]); // bool = true
        let bytes = subblock_with(3, &content);
        let mut r = Reader::new(&bytes);
        let lww = r.read_lww_bool(3).unwrap();
        assert_eq!(lww.timestamp, CrdtId { author: 5, seq: 7 });
        assert!(lww.value);
    }

    #[test]
    fn lww_string_round_trip() {
        // outer subblock(2) ->
        //   tag(1,Id)+id(0,0) + tag(2,Length4)+inner_subblock("hi")
        let mut inner = Vec::new();
        inner.extend_from_slice(&[0x02, 0x01]); // varuint len=2, is_ascii=1
        inner.extend_from_slice(b"hi");
        let mut content = vec![0x1F, 0x00, 0x00]; // timestamp = SENTINEL
        content.push(0x2C); // tag(2, Length4)
        let inner_len = u32::try_from(inner.len()).unwrap();
        content.extend_from_slice(&inner_len.to_le_bytes());
        content.extend_from_slice(&inner);

        let bytes = subblock_with(2, &content);
        let mut r = Reader::new(&bytes);
        let lww = r.read_lww_string(2).unwrap();
        assert_eq!(lww.timestamp, CrdtId::SENTINEL);
        assert_eq!(lww.value, "hi");
    }
}
