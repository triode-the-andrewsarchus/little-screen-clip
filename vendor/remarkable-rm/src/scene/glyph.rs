//! Highlight rectangles attached to PDF or typed text.
use crate::{error::ParseError, primitives::Reader, scene::line::PenColor, tag::TagType};

#[derive(Debug, Clone)]
pub struct Rectangle {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
pub struct GlyphRange {
    pub text: String,
    pub color: PenColor,
    pub color_rgba: Option<[u8; 4]>,
    pub rectangles: Vec<Rectangle>,
}

pub fn read_glyph(body: &mut Reader<'_>) -> Result<GlyphRange, ParseError> {
    for index in [2, 3] {
        if body
            .peek_tag()
            .is_some_and(|t| t.index == index && t.tag_type == TagType::Byte4)
        {
            body.read_int(index)?;
        }
    }
    let color = PenColor::from_u32(body.read_int(4)?)?;
    let text = body.read_string_subblock(5)?.to_owned();
    let mut sub = body.read_subblock(6)?;
    let count = sub.read_varuint()?;
    let mut rectangles = Vec::new();
    for _ in 0..count {
        rectangles.push(Rectangle {
            x: sub.read_f64()?,
            y: sub.read_f64()?,
            width: sub.read_f64()?,
            height: sub.read_f64()?,
        });
    }
    for index in [7, 8] {
        if body
            .peek_tag()
            .is_some_and(|t| t.index == index && t.tag_type == TagType::Id)
        {
            body.read_id(index)?;
        }
    }
    if body
        .peek_tag()
        .is_some_and(|t| t.index == 9 && t.tag_type == TagType::Byte1)
    {
        body.read_byte(9)?;
    }
    let color_rgba = if body
        .peek_tag()
        .is_some_and(|t| t.index == 10 && t.tag_type == TagType::Byte4)
    {
        let [b, g, r, a] = body.read_int(10)?.to_le_bytes();
        Some([r, g, b, a])
    } else {
        None
    };
    Ok(GlyphRange {
        text,
        color,
        color_rgba,
        rectangles,
    })
}
