//! Stroke (line) format — spec §7.

use crate::crdt::CrdtId;
use crate::error::ParseError;
use crate::primitives::Reader;
use crate::tag::TagType;

/// Pen tools — spec §7.2. Values 9-11, 19-20, and 22 are reserved/invalid
/// and rejected by [`Pen::from_u32`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum Pen {
    PaintbrushV1 = 0,
    PencilV1 = 1,
    BallpointV1 = 2,
    MarkerV1 = 3,
    FinelinerV1 = 4,
    HighlighterV1 = 5,
    Eraser = 6,
    MechanicalPencilV1 = 7,
    EraserAreaSelect = 8,
    PaintbrushV2 = 12,
    MechanicalPencilV2 = 13,
    PencilV2 = 14,
    BallpointV2 = 15,
    MarkerV2 = 16,
    FinelinerV2 = 17,
    HighlighterV2 = 18,
    Calligraphy = 21,
    Shader = 23,
}

impl Pen {
    pub fn from_u32(v: u32) -> Result<Self, ParseError> {
        Ok(match v {
            0 => Self::PaintbrushV1,
            1 => Self::PencilV1,
            2 => Self::BallpointV1,
            3 => Self::MarkerV1,
            4 => Self::FinelinerV1,
            5 => Self::HighlighterV1,
            6 => Self::Eraser,
            7 => Self::MechanicalPencilV1,
            8 => Self::EraserAreaSelect,
            12 => Self::PaintbrushV2,
            13 => Self::MechanicalPencilV2,
            14 => Self::PencilV2,
            15 => Self::BallpointV2,
            16 => Self::MarkerV2,
            17 => Self::FinelinerV2,
            18 => Self::HighlighterV2,
            21 => Self::Calligraphy,
            23 => Self::Shader,
            other => return Err(ParseError::InvalidPen(other)),
        })
    }
}

/// Pen colors — spec §7.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum PenColor {
    Black = 0,
    Gray = 1,
    White = 2,
    Yellow = 3,
    Green = 4,
    Pink = 5,
    Blue = 6,
    Red = 7,
    GrayOverlap = 8,
    Highlight = 9,
    GreenV2 = 10,
    Cyan = 11,
    Magenta = 12,
    YellowV2 = 13,
}

impl PenColor {
    pub fn from_u32(v: u32) -> Result<Self, ParseError> {
        Ok(match v {
            0 => Self::Black,
            1 => Self::Gray,
            2 => Self::White,
            3 => Self::Yellow,
            4 => Self::Green,
            5 => Self::Pink,
            6 => Self::Blue,
            7 => Self::Red,
            8 => Self::GrayOverlap,
            9 => Self::Highlight,
            10 => Self::GreenV2,
            11 => Self::Cyan,
            12 => Self::Magenta,
            13 => Self::YellowV2,
            other => return Err(ParseError::InvalidPenColor(other)),
        })
    }
}

/// Single stroke point. On disk this is one of two physical layouts (v1 = 24
/// bytes, v2 = 14 bytes; spec §7.1). The parser normalizes both to this
/// in-memory shape — v1 floats are scaled into the v2 integer ranges so the
/// renderer can stay single-codepath.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub speed: u16,
    pub width: u16,
    pub direction: u8,
    pub pressure: u8,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub tool: Pen,
    pub color: PenColor,
    /// Optional explicit RGBA colour (wire order is BGRA), firmware 3.x.
    pub color_rgba: Option<[u8; 4]>,
    pub thickness_scale: f64,
    pub starting_length: f32,
    pub points: Vec<Point>,
    pub timestamp: CrdtId,
    pub move_id: Option<CrdtId>,
}

const POINT_SIZE_V1: usize = 24;
const POINT_SIZE_V2: usize = 14;

/// Read a `Line` value (spec §7) from the body of a `SceneLineItemBlock`'s
/// value sub-block. The caller has already consumed the `item_type` byte.
pub fn read_line(body: &mut Reader<'_>, current_version: u8) -> Result<Line, ParseError> {
    let tool = Pen::from_u32(body.read_int(1)?)?;
    let color = PenColor::from_u32(body.read_int(2)?)?;
    let thickness_scale = body.read_double(3)?;
    let starting_length = body.read_float(4)?;

    let mut points_sub = body.read_subblock(5)?;
    let points = read_points(&mut points_sub, current_version)?;

    let timestamp = body.read_id(6)?;
    // `move_id` is optional, and newer firmwares may emit additional unknown
    // tags after it; we read it only if the next tag actually matches.
    let move_id = match body.peek_tag() {
        Some(t) if t.index == 7 && t.tag_type == TagType::Id => Some(body.read_id(7)?),
        _ => None,
    };
    let color_rgba = match body.peek_tag() {
        Some(tag) if tag.index == 8 && tag.tag_type == TagType::Byte4 => {
            let [blue, green, red, alpha] = body.read_int(8)?.to_le_bytes();
            Some([red, green, blue, alpha])
        }
        _ => None,
    };

    Ok(Line {
        tool,
        color,
        color_rgba,
        thickness_scale,
        starting_length,
        points,
        timestamp,
        move_id,
    })
}

fn read_points(reader: &mut Reader<'_>, current_version: u8) -> Result<Vec<Point>, ParseError> {
    let point_size = if current_version <= 1 {
        POINT_SIZE_V1
    } else {
        POINT_SIZE_V2
    };
    let total = reader.remaining();
    if !total.is_multiple_of(point_size) {
        return Err(ParseError::InvalidBlock(
            "point data length is not a multiple of point size",
        ));
    }
    let count = total / point_size;
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let p = if current_version <= 1 {
            read_point_v1(reader)?
        } else {
            read_point_v2(reader)?
        };
        points.push(p);
    }
    Ok(points)
}

fn read_point_v1(reader: &mut Reader<'_>) -> Result<Point, ParseError> {
    let x = reader.read_f32()?;
    let y = reader.read_f32()?;
    let speed_raw = reader.read_f32()?;
    let direction_raw = reader.read_f32()?;
    let width_raw = reader.read_f32()?;
    let pressure_raw = reader.read_f32()?;

    Ok(Point {
        x,
        y,
        speed: scale_to_u16(speed_raw * 4.0),
        direction: scale_to_u8(direction_raw * 255.0 / std::f32::consts::TAU),
        width: scale_to_u16(width_raw * 4.0),
        pressure: scale_to_u8(pressure_raw * 255.0),
    })
}

fn read_point_v2(reader: &mut Reader<'_>) -> Result<Point, ParseError> {
    let x = reader.read_f32()?;
    let y = reader.read_f32()?;
    let speed = reader.read_u16()?;
    let width = reader.read_u16()?;
    let direction = reader.read_u8()?;
    let pressure = reader.read_u8()?;
    Ok(Point {
        x,
        y,
        speed,
        width,
        direction,
        pressure,
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scale_to_u16(v: f32) -> u16 {
    let clamped = v.clamp(0.0, f32::from(u16::MAX));
    if clamped.is_nan() {
        0
    } else {
        clamped.round() as u16
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scale_to_u8(v: f32) -> u8 {
    let clamped = v.clamp(0.0, f32::from(u8::MAX));
    if clamped.is_nan() {
        0
    } else {
        clamped.round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the value-sub-block content for a Line (without the leading
    /// `item_type` byte — the caller has already consumed that).
    fn line_bytes(with_move_id: bool, points_bytes: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        // tag 1, Byte4: tool = Fineliner v1 (4)
        bytes.extend_from_slice(&[0x14]);
        bytes.extend_from_slice(&4u32.to_le_bytes());
        // tag 2, Byte4: color = Black (0)
        bytes.extend_from_slice(&[0x24]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        // tag 3, Byte8: thickness_scale = 2.0
        bytes.extend_from_slice(&[0x38]);
        bytes.extend_from_slice(&2.0_f64.to_le_bytes());
        // tag 4, Byte4: starting_length = 0.0
        bytes.extend_from_slice(&[0x44]);
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
        // tag 5, Length4: points subblock
        bytes.extend_from_slice(&[0x5C]);
        let len = u32::try_from(points_bytes.len()).unwrap();
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(points_bytes);
        // tag 6, Id: timestamp = (1, 1)
        bytes.extend_from_slice(&[0x6F, 0x01, 0x01]);
        if with_move_id {
            // tag 7, Id: move_id = (1, 2)
            bytes.extend_from_slice(&[0x7F, 0x01, 0x02]);
        }
        bytes
    }

    #[test]
    fn parses_v2_line_two_points() {
        // Two v2 points: (10.0, 20.0, speed=100, width=4, direction=128, pressure=200)
        //                (15.0, 25.0, speed=110, width=5, direction=130, pressure=210)
        let mut points = Vec::new();
        for &(x, y, speed, width, direction, pressure) in &[
            (10.0_f32, 20.0_f32, 100u16, 4u16, 128u8, 200u8),
            (15.0_f32, 25.0_f32, 110u16, 5u16, 130u8, 210u8),
        ] {
            points.extend_from_slice(&x.to_le_bytes());
            points.extend_from_slice(&y.to_le_bytes());
            points.extend_from_slice(&speed.to_le_bytes());
            points.extend_from_slice(&width.to_le_bytes());
            points.push(direction);
            points.push(pressure);
        }
        let bytes = line_bytes(false, &points);
        let mut r = Reader::new(&bytes);
        let line = read_line(&mut r, 2).unwrap();
        assert_eq!(line.tool, Pen::FinelinerV1);
        assert_eq!(line.color, PenColor::Black);
        assert!((line.thickness_scale - 2.0).abs() < f64::EPSILON);
        assert_eq!(line.points.len(), 2);
        assert!((line.points[0].x - 10.0).abs() < f32::EPSILON);
        assert_eq!(line.points[0].speed, 100);
        assert_eq!(line.points[1].pressure, 210);
        assert!(line.move_id.is_none());
    }

    #[test]
    fn parses_v1_line_with_scaling() {
        // One v1 point: x=1.0, y=2.0, speed=25 (-> u16 100), direction=π (-> u8 ~127.5),
        // width=2.0 (-> u16 8), pressure=1.0 (-> u8 255)
        let mut points = Vec::new();
        points.extend_from_slice(&1.0_f32.to_le_bytes());
        points.extend_from_slice(&2.0_f32.to_le_bytes());
        points.extend_from_slice(&25.0_f32.to_le_bytes());
        points.extend_from_slice(&std::f32::consts::PI.to_le_bytes());
        points.extend_from_slice(&2.0_f32.to_le_bytes());
        points.extend_from_slice(&1.0_f32.to_le_bytes());
        let bytes = line_bytes(false, &points);
        let mut r = Reader::new(&bytes);
        let line = read_line(&mut r, 1).unwrap();
        assert_eq!(line.points.len(), 1);
        let p = line.points[0];
        assert_eq!(p.speed, 100);
        assert_eq!(p.width, 8);
        assert_eq!(p.pressure, 255);
        // direction ≈ pi * 255 / (2π) = 127.5 → rounds to 128
        assert_eq!(p.direction, 128);
    }

    #[test]
    fn parses_line_with_move_id() {
        let mut points = Vec::new();
        points.extend_from_slice(&0.0_f32.to_le_bytes());
        points.extend_from_slice(&0.0_f32.to_le_bytes());
        points.extend_from_slice(&[0u8; 6]); // speed, width, direction, pressure
        let bytes = line_bytes(true, &points);
        let mut r = Reader::new(&bytes);
        let line = read_line(&mut r, 2).unwrap();
        assert_eq!(line.move_id, Some(CrdtId { author: 1, seq: 2 }));
    }

    #[test]
    fn explicit_colour_decodes_bgra_without_changing_palette_identity() {
        let mut bytes = line_bytes(true, &[]);
        bytes.extend_from_slice(&[0x84, 0x01, 0x10, 0x20, 0x30, 0x40]);
        let line = read_line(&mut Reader::new(&bytes), 2).unwrap();
        assert_eq!(line.color, PenColor::Black);
        assert_eq!(line.color_rgba, Some([0x30, 0x20, 0x10, 0x40]));
        assert_eq!(line.move_id, Some(CrdtId { author: 1, seq: 2 }));
    }
}
