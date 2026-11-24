//! Tag-length-value reader — spec §3.

use crate::crdt::CrdtId;
use crate::error::ParseError;
use crate::primitives::Reader;

/// Tag type discriminator (low nibble of the tag varuint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TagType {
    Byte1 = 0x01,
    Byte4 = 0x04,
    Byte8 = 0x08,
    Length4 = 0x0C,
    Id = 0x0F,
}

#[derive(Debug, Clone, Copy)]
pub struct Tag {
    pub index: u32,
    pub tag_type: TagType,
}

impl<'a> Reader<'a> {
    /// Peek the next tag without advancing the cursor. Returns `None` if the
    /// reader is empty or the next bytes don't form a valid tag.
    #[must_use]
    pub fn peek_tag(&self) -> Option<Tag> {
        self.clone().read_tag().ok()
    }

    pub fn read_tag(&mut self) -> Result<Tag, ParseError> {
        let value = self.read_varuint()?;
        let raw_type = value & 0x0F;
        let raw_index = value >> 4;
        let index = u32::try_from(raw_index).map_err(|_| ParseError::VarUIntOverflow)?;
        let tag_type = match raw_type {
            0x01 => TagType::Byte1,
            0x04 => TagType::Byte4,
            0x08 => TagType::Byte8,
            0x0C => TagType::Length4,
            0x0F => TagType::Id,
            #[allow(clippy::cast_possible_truncation)] // raw_type is masked to 0..=15
            other => {
                return Err(ParseError::UnknownTagType {
                    index,
                    tag_type: other as u8,
                });
            }
        };
        Ok(Tag { index, tag_type })
    }

    pub fn expect_tag(
        &mut self,
        expected_index: u32,
        expected_type: TagType,
    ) -> Result<Tag, ParseError> {
        let tag = self.read_tag()?;
        if tag.index != expected_index || tag.tag_type != expected_type {
            return Err(ParseError::UnexpectedTag {
                expected_index,
                expected_type: expected_type as u8,
                got_index: tag.index,
                got_type: tag.tag_type as u8,
            });
        }
        Ok(tag)
    }

    pub fn read_id(&mut self, index: u32) -> Result<CrdtId, ParseError> {
        self.expect_tag(index, TagType::Id)?;
        self.read_crdt_id()
    }

    pub fn read_byte(&mut self, index: u32) -> Result<u8, ParseError> {
        self.expect_tag(index, TagType::Byte1)?;
        self.read_u8()
    }

    pub fn read_bool_field(&mut self, index: u32) -> Result<bool, ParseError> {
        self.expect_tag(index, TagType::Byte1)?;
        self.read_bool()
    }

    pub fn read_int(&mut self, index: u32) -> Result<u32, ParseError> {
        self.expect_tag(index, TagType::Byte4)?;
        self.read_u32()
    }

    pub fn read_float(&mut self, index: u32) -> Result<f32, ParseError> {
        self.expect_tag(index, TagType::Byte4)?;
        self.read_f32()
    }

    pub fn read_double(&mut self, index: u32) -> Result<f64, ParseError> {
        self.expect_tag(index, TagType::Byte8)?;
        self.read_f64()
    }

    /// Open a length-prefixed sub-block at `index` and return a bounded reader
    /// scoped to its content.
    pub fn read_subblock(&mut self, index: u32) -> Result<Reader<'a>, ParseError> {
        self.expect_tag(index, TagType::Length4)?;
        let length = usize::try_from(self.read_u32()?)
            .map_err(|_| ParseError::InvalidBlock("subblock length overflows usize"))?;
        self.read_bounded(length)
    }

    /// Read a string-shaped sub-block at `index` per spec §8.7:
    /// `varuint string_length, bool is_ascii, [string_length bytes UTF-8]`.
    pub fn read_string_subblock(&mut self, index: u32) -> Result<&'a str, ParseError> {
        let mut sub = self.read_subblock(index)?;
        let str_len = usize::try_from(sub.read_varuint()?)
            .map_err(|_| ParseError::InvalidBlock("string length overflows usize"))?;
        let _is_ascii = sub.read_bool()?;
        let bytes = sub.read_bytes(str_len)?;
        std::str::from_utf8(bytes).map_err(|_| ParseError::InvalidUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec Appendix B examples.
    #[test]
    fn appendix_b_tag_decoding() {
        let cases = [
            (0x1F, 1, TagType::Id),
            (0x21, 2, TagType::Byte1),
            (0x24, 2, TagType::Byte4),
            (0x38, 3, TagType::Byte8),
            (0x5C, 5, TagType::Length4),
            (0x6C, 6, TagType::Length4),
        ];
        for (encoded, index, ty) in cases {
            let bytes = [encoded];
            let mut r = Reader::new(&bytes);
            let tag = r.read_tag().unwrap();
            assert_eq!(tag.index, index);
            assert_eq!(tag.tag_type, ty);
        }
    }

    #[test]
    fn unknown_tag_type_errors() {
        let mut r = Reader::new(&[0x12]); // index=1, type=0x2 (unknown)
        assert!(matches!(
            r.read_tag(),
            Err(ParseError::UnknownTagType {
                index: 1,
                tag_type: 0x02
            })
        ));
    }

    #[test]
    fn expect_tag_mismatch() {
        let mut r = Reader::new(&[0x1F]); // index=1, Id
        let err = r.expect_tag(2, TagType::Id).unwrap_err();
        assert!(matches!(
            err,
            ParseError::UnexpectedTag {
                expected_index: 2,
                got_index: 1,
                ..
            }
        ));
    }

    #[test]
    fn read_subblock_carves_bounded_reader() {
        // tag(5, Length4)=0x5C, length=2 (LE u32), then content [0xAA 0xBB], then trailing 0xFF
        let bytes = [0x5C, 0x02, 0x00, 0x00, 0x00, 0xAA, 0xBB, 0xFF];
        let mut r = Reader::new(&bytes);
        let mut sub = r.read_subblock(5).unwrap();
        assert_eq!(sub.read_u8().unwrap(), 0xAA);
        assert_eq!(sub.read_u8().unwrap(), 0xBB);
        assert!(sub.is_eof());
        // Parent picks up at the trailing byte.
        assert_eq!(r.read_u8().unwrap(), 0xFF);
    }

    #[test]
    fn read_string_subblock_decodes_utf8() {
        // tag(2, Length4)=0x2C, length=7, content: varuint(5)=0x05, bool=0x01, "hello"
        let mut bytes = vec![0x2C, 0x07, 0x00, 0x00, 0x00, 0x05, 0x01];
        bytes.extend_from_slice(b"hello");
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_string_subblock(2).unwrap(), "hello");
    }
}
