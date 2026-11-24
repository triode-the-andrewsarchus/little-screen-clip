//! Top-level block envelope — spec §4 (header) and §5 (block types).

use crate::error::ParseError;
use crate::primitives::Reader;

/// File header magic — exactly 43 bytes (spec §1).
pub const FILE_HEADER: &[u8; 43] = b"reMarkable .lines file, version=6          ";

/// Prefix shared by every `.rm` header version (spec §1, §14).
const HEADER_PREFIX: &[u8] = b"reMarkable .lines file, version=";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockType {
    MigrationInfo = 0x00,
    SceneTree = 0x01,
    TreeNode = 0x02,
    SceneGlyphItem = 0x03,
    SceneGroupItem = 0x04,
    SceneLineItem = 0x05,
    SceneTextItem = 0x06,
    RootText = 0x07,
    SceneTombstoneItem = 0x08,
    AuthorIds = 0x09,
    PageInfo = 0x0A,
    SceneInfo = 0x0D,
}

impl BlockType {
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            0x00 => Self::MigrationInfo,
            0x01 => Self::SceneTree,
            0x02 => Self::TreeNode,
            0x03 => Self::SceneGlyphItem,
            0x04 => Self::SceneGroupItem,
            0x05 => Self::SceneLineItem,
            0x06 => Self::SceneTextItem,
            0x07 => Self::RootText,
            0x08 => Self::SceneTombstoneItem,
            0x09 => Self::AuthorIds,
            0x0A => Self::PageInfo,
            0x0D => Self::SceneInfo,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BlockHeader {
    pub length: u32,
    pub min_version: u8,
    pub current_version: u8,
    pub block_type: u8,
}

impl BlockHeader {
    #[must_use]
    pub const fn kind(&self) -> Option<BlockType> {
        BlockType::from_byte(self.block_type)
    }
}

#[derive(Debug)]
pub struct Block<'a> {
    pub header: BlockHeader,
    pub body: Reader<'a>,
}

/// Validate the v6 magic and return an iterator yielding every top-level
/// block in `data`.
pub fn iter_blocks(data: &[u8]) -> Result<BlockIter<'_>, ParseError> {
    let mut reader = Reader::new(data);
    read_file_header(&mut reader)?;
    Ok(BlockIter { reader })
}

pub fn read_file_header(reader: &mut Reader<'_>) -> Result<(), ParseError> {
    let bytes = reader.read_bytes(FILE_HEADER.len())?;
    if bytes == FILE_HEADER.as_slice() {
        return Ok(());
    }
    if let Some(&digit) = bytes.get(HEADER_PREFIX.len())
        && bytes.starts_with(HEADER_PREFIX)
        && digit.is_ascii_digit()
        && digit != b'6'
    {
        return Err(ParseError::UnsupportedVersion(digit - b'0'));
    }
    Err(ParseError::BadMagic)
}

#[derive(Debug)]
pub struct BlockIter<'a> {
    reader: Reader<'a>,
}

impl<'a> Iterator for BlockIter<'a> {
    type Item = Result<Block<'a>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.reader.is_eof() {
            return None;
        }
        Some(read_block(&mut self.reader))
    }
}

fn read_block<'a>(reader: &mut Reader<'a>) -> Result<Block<'a>, ParseError> {
    let length = reader.read_u32()?;
    let _unknown = reader.read_u8()?;
    let min_version = reader.read_u8()?;
    let current_version = reader.read_u8()?;
    let block_type = reader.read_u8()?;
    let length_usize = usize::try_from(length)
        .map_err(|_| ParseError::InvalidBlock("block length overflows usize"))?;
    let body = reader.read_bounded(length_usize)?;
    Ok(Block {
        header: BlockHeader {
            length,
            min_version,
            current_version,
            block_type,
        },
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec Appendix C: minimal `MigrationInfo` file.
    fn appendix_c_bytes() -> Vec<u8> {
        let mut bytes = FILE_HEADER.to_vec();
        // block: length=5, unknown=0, min=1, cur=1, type=0
        bytes.extend_from_slice(&[0x05, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00]);
        // content: tag(1,Id)+id(1,1) + tag(2,Byte1)+true
        bytes.extend_from_slice(&[0x1F, 0x01, 0x01, 0x21, 0x01]);
        bytes
    }

    #[test]
    fn appendix_c_iterates_one_block() {
        let bytes = appendix_c_bytes();
        let blocks: Vec<_> = iter_blocks(&bytes).unwrap().collect();
        assert_eq!(blocks.len(), 1);
        let block = blocks.into_iter().next().unwrap().unwrap();
        assert_eq!(block.header.length, 5);
        assert_eq!(block.header.min_version, 1);
        assert_eq!(block.header.current_version, 1);
        assert_eq!(block.header.kind(), Some(BlockType::MigrationInfo));
        assert_eq!(block.body.remaining(), 5);
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut bytes = b"NotAReMarkableFile_____________________________".to_vec();
        bytes.truncate(43);
        assert!(matches!(iter_blocks(&bytes), Err(ParseError::BadMagic)));
    }

    #[test]
    fn detects_unsupported_version() {
        // version=3 then padding to 43 bytes
        let mut bytes = b"reMarkable .lines file, version=3          ".to_vec();
        bytes.truncate(43);
        assert!(matches!(
            iter_blocks(&bytes),
            Err(ParseError::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn unknown_block_type_returns_none_from_kind() {
        let mut bytes = FILE_HEADER.to_vec();
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0xFE]);
        let block = iter_blocks(&bytes).unwrap().next().unwrap().unwrap();
        assert_eq!(block.header.kind(), None);
        assert_eq!(block.header.block_type, 0xFE);
    }
}
