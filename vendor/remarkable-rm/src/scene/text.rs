//! `RootTextBlock` (0x07) and the text-CRDT format — spec §8.

use crate::crdt::CrdtId;
use crate::error::ParseError;
use crate::primitives::Reader;

/// Inline formatting code — spec §8.4. Lives on a text item with an empty
/// string body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum InlineFormat {
    BoldStart = 1,
    BoldEnd = 2,
    ItalicStart = 3,
    ItalicEnd = 4,
}

impl InlineFormat {
    pub fn from_u32(v: u32) -> Result<Self, ParseError> {
        Ok(match v {
            1 => Self::BoldStart,
            2 => Self::BoldEnd,
            3 => Self::ItalicStart,
            4 => Self::ItalicEnd,
            other => return Err(ParseError::InvalidInlineFormat(other)),
        })
    }
}

/// Paragraph style — spec §8.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ParagraphStyle {
    Basic = 0,
    Plain = 1,
    Heading = 2,
    Bold = 3,
    Bullet = 4,
    Bullet2 = 5,
    CheckboxUnchecked = 6,
    CheckboxChecked = 7,
    Numbered = 10,
}

impl ParagraphStyle {
    pub fn from_u8(v: u8) -> Result<Self, ParseError> {
        Ok(match v {
            0 => Self::Basic,
            1 => Self::Plain,
            2 => Self::Heading,
            3 => Self::Bold,
            4 => Self::Bullet,
            5 => Self::Bullet2,
            6 => Self::CheckboxUnchecked,
            7 => Self::CheckboxChecked,
            10 => Self::Numbered,
            other => return Err(ParseError::InvalidParagraphStyle(other)),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TextItem {
    pub item_id: CrdtId,
    pub left_id: CrdtId,
    pub right_id: CrdtId,
    pub deleted_length: u32,
    pub content: Option<TextContent>,
}

#[derive(Debug, Clone)]
pub enum TextContent {
    String(String),
    Format(InlineFormat),
}

#[derive(Debug, Clone)]
pub struct ParagraphFormat {
    pub char_id: CrdtId,
    pub timestamp: CrdtId,
    pub style: ParagraphStyle,
}

#[derive(Debug, Clone)]
pub struct RootText {
    pub items: Vec<TextItem>,
    pub paragraphs: Vec<ParagraphFormat>,
    pub pos_x: f64,
    pub pos_y: f64,
    pub width: f32,
}

/// Parse a `RootTextBlock` body — spec §8.1.
pub fn read_root_text_block(body: &mut Reader<'_>) -> Result<RootText, ParseError> {
    let _block_id = body.read_id(1)?; // always CrdtId(0, 0) per spec

    let mut content = body.read_subblock(2)?;

    // Text items: subblock(1) → subblock(1) → varuint count + items.
    let mut items_outer = content.read_subblock(1)?;
    let mut items_inner = items_outer.read_subblock(1)?;
    let num_items = bounded_count(&mut items_inner, "text item count")?;
    let mut items = Vec::with_capacity(num_items);
    for _ in 0..num_items {
        items.push(read_text_item(&mut items_inner)?);
    }

    // Paragraph styles: subblock(2) → subblock(1) → varuint count + formats.
    let mut formats_outer = content.read_subblock(2)?;
    let mut formats_inner = formats_outer.read_subblock(1)?;
    let num_formats = bounded_count(&mut formats_inner, "paragraph format count")?;
    let mut paragraphs = Vec::with_capacity(num_formats);
    for _ in 0..num_formats {
        paragraphs.push(read_paragraph_format(&mut formats_inner)?);
    }

    let mut pos_sub = body.read_subblock(3)?;
    let pos_x = pos_sub.read_f64()?;
    let pos_y = pos_sub.read_f64()?;

    let width = body.read_float(4)?;

    Ok(RootText {
        items,
        paragraphs,
        pos_x,
        pos_y,
        width,
    })
}

fn bounded_count(r: &mut Reader<'_>, what: &'static str) -> Result<usize, ParseError> {
    usize::try_from(r.read_varuint()?).map_err(|_| ParseError::InvalidBlock(what))
}

fn read_text_item(body: &mut Reader<'_>) -> Result<TextItem, ParseError> {
    let mut item = body.read_subblock(0)?;
    let item_id = item.read_id(2)?;
    let left_id = item.read_id(3)?;
    let right_id = item.read_id(4)?;
    let deleted_length = item.read_int(5)?;

    let content = if item.is_eof() {
        None
    } else {
        let mut sub = item.read_subblock(6)?;
        let str_len = bounded_count(&mut sub, "text string length")?;
        let _is_ascii = sub.read_bool()?;
        let bytes = sub.read_bytes(str_len)?;
        if sub
            .peek_tag()
            .is_some_and(|tag| tag.index == 2 && tag.tag_type == crate::tag::TagType::Byte4)
        {
            Some(TextContent::Format(InlineFormat::from_u32(
                sub.read_int(2)?,
            )?))
        } else {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| ParseError::InvalidUtf8)?
                .to_owned();
            Some(TextContent::String(text))
        }
    };

    Ok(TextItem {
        item_id,
        left_id,
        right_id,
        deleted_length,
        content,
    })
}

fn read_paragraph_format(body: &mut Reader<'_>) -> Result<ParagraphFormat, ParseError> {
    let char_id = body.read_crdt_id()?;
    let timestamp = body.read_id(1)?;
    let mut sub = body.read_subblock(2)?;
    let style = ParagraphStyle::from_u8(sub.read_byte(1)?)?;
    Ok(ParagraphFormat {
        char_id,
        timestamp,
        style,
    })
}
