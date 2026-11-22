use oxideav_embroidery::{Command, hus};

use super::{Bytes, Move, Plot, Result, inverted_delta, palettes};
use crate::embroidery::MAX_COMMANDS;

pub(super) fn exp(data: &[u8], plot: &mut Plot<'_>) -> Result {
    if data.len() < 4 || !data.len().is_multiple_of(2) {
        return Err("Incomplete EXP commands".into());
    }
    pairs(Bytes(data), PairFormat::Exp, plot)
}

pub(super) fn stx(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let offset = Bytes::at(data, 0x14)?.le32()?;
    pairs(Bytes::at(data, offset)?, PairFormat::Exp, plot)
}

pub(super) fn jef(data: &[u8], plot: &mut Plot<'_>) -> Result {
    if data.get(4..8) == Some(&1_u32.to_le_bytes()) {
        return jef_plus(data, plot);
    }
    let offset = Bytes(data).le32()?;
    let count = Bytes::at(data, 24)?.le32()?;
    let palette = data.get(0x74..offset).ok_or("Invalid JEF stitch offset")?;
    let mut palette = Bytes(palette);
    // Validate before allocation and never accept a palette that overlaps stitches.
    if count > palette.0.len() / 4 || count > u16::MAX as usize {
        return Err("Invalid JEF thread count".into());
    }
    for _ in 0..count {
        let index = i32::from_le_bytes(palette.array()?).unsigned_abs() as usize;
        plot.palette.push(palettes::color(palettes::JEF, index));
    }
    pairs(Bytes::at(data, offset)?, PairFormat::Jef, plot)
}

pub(super) fn jef_plus(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let offset = Bytes(data).le32()?;
    if !oxideav_embroidery::jef::probe(data) || data.len().saturating_sub(offset) > MAX_COMMANDS * 2
    {
        return Err("Invalid or oversized JEF+ stitch stream".into());
    }
    let file = oxideav_embroidery::jef::decode(data).map_err(|error| error.to_string())?;
    oxide_commands(file.design.commands, plot)
}

pub(super) fn sew(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let mut reader = Bytes(data.get(..0x1d78).ok_or("Truncated SEW header")?);
    let count = reader.le16()?;
    for _ in 0..count {
        let index = reader.le16()?;
        plot.palette.push(palettes::color(palettes::SEW, index));
    }
    pairs(Bytes::at(data, 0x1d78)?, PairFormat::Sew, plot)
}

#[derive(Clone, Copy)]
enum PairFormat {
    Exp,
    Jef,
    Sew,
}

impl PairFormat {
    fn control(self, control: u8) -> Option<PairCommand> {
        use PairCommand::{Change, MoveTo, Trim};
        match (self, control) {
            (Self::Exp, 0x80) => Some(Trim),
            (Self::Jef | Self::Sew, 0x02) | (Self::Exp | Self::Sew, 0x04) => {
                Some(MoveTo(Move::Jump))
            }
            (Self::Exp, 0x02) | (Self::Sew, 0x10) => Some(MoveTo(Move::Sew)),
            (Self::Exp | Self::Jef, 0x01) => Some(Change),
            (Self::Sew, odd) if odd & 1 != 0 => Some(Change),
            _ => None,
        }
    }
}

enum PairCommand {
    MoveTo(Move),
    Change,
    Trim,
}

fn pairs(mut input: Bytes<'_>, format: PairFormat, plot: &mut Plot<'_>) -> Result {
    while input.0.len() >= 2 {
        plot.record()?;
        let pair = input.array()?;
        let [x, control] = pair;
        if x != 0x80 {
            let (x, y) = inverted_delta(pair);
            plot.relative(Move::Sew, x, y);
            continue;
        }
        let Some(command) = format.control(control) else {
            break;
        };
        if input.0.len() < 2 {
            break;
        }
        let (x, y) = inverted_delta(input.array()?);
        match command {
            PairCommand::MoveTo(movement) => plot.relative(movement, x, y),
            PairCommand::Trim => plot.trim(),
            PairCommand::Change => match format {
                PairFormat::Exp => {
                    plot.change();
                    if (x, y) != (0, 0) {
                        plot.relative(Move::Jump, x, y);
                    }
                }
                PairFormat::Sew => plot.change(),
                PairFormat::Jef => {
                    // Index zero in a JEF palette requests a stop without
                    // changing thread or breaking sewing continuity.
                    let index = plot.color_index + 1;
                    if plot.palette.get(index) == Some(&None) {
                        plot.palette[index] = plot.color(plot.color_index);
                        plot.color_index = index;
                    } else {
                        plot.change();
                    }
                }
            },
        }
    }
    Ok(())
}

pub(super) fn hus(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let HusStreams {
        records,
        palette,
        streams: [commands, xs, ys],
    } = hus_streams(data)?;
    plot.palette = palette
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&index| palettes::color(palettes::HUS, usize::from(u16::from_le_bytes(index))))
        .collect();
    let decode = |stream: &[u8]| {
        super::check_cancelled(plot.cancelled)?;
        oxideav_embroidery::gl::decompress(stream, records)
            .map(|(bytes, _)| bytes)
            .map_err(|error| error.to_string())
    };
    let (commands, xs, ys) = (decode(commands)?, decode(xs)?, decode(ys)?);
    for ((command, x), y) in commands.into_iter().zip(xs).zip(ys) {
        plot.record()?;
        let (x, y) = inverted_delta([x, y]);
        match command {
            hus::ATTR_STITCH => plot.relative(Move::Sew, x, y),
            hus::ATTR_JUMP | hus::ATTR_TRIM => plot.relative(Move::Jump, x, y),
            hus::ATTR_COLOR_CHANGE => {
                plot.relative(Move::Shift, x, y);
                plot.change();
            }
            _ => break,
        }
    }
    Ok(())
}

struct HusStreams<'a> {
    records: usize,
    palette: &'a [u8],
    streams: [&'a [u8]; 3],
}

fn hus_streams(data: &[u8]) -> Result<HusStreams<'_>> {
    const PE_DESIGN_HUS: u32 = 0x00c8_fc5d;
    let mut header = Bytes(
        data.strip_prefix(&hus::MAGIC.to_le_bytes())
            .or_else(|| data.strip_prefix(&PE_DESIGN_HUS.to_le_bytes()))
            .ok_or("Missing HUS signature")?,
    );
    let [records, colors] = header.le32s()?;
    if records > MAX_COMMANDS || colors > 1000 {
        return Err("Embroidery stitch budget exceeded".into());
    }
    header.skip(8)?;
    let [commands, x, y] = header.le32s()?;
    let view = |range| data.get(range).ok_or("Invalid HUS stream offset");
    let mut palette = Bytes(view(0x2a..commands)?);
    Ok(HusStreams {
        records,
        palette: palette.take(colors * 2)?,
        streams: [view(commands..x)?, view(x..y)?, view(y..data.len())?],
    })
}

pub(super) fn vip(data: &[u8], plot: &mut Plot<'_>) -> Result {
    let file = hus::parse_vip(data).map_err(|error| error.to_string())?;
    compressed(&file, plot)
}

fn compressed(file: &hus::HusFile, plot: &mut Plot<'_>) -> Result {
    if file.stitch_count as usize > MAX_COMMANDS {
        return Err("Embroidery stitch budget exceeded".into());
    }
    super::check_cancelled(plot.cancelled)?;
    let design = hus::decode_design(file).map_err(|error| error.to_string())?;
    oxide_commands(design.commands, plot)
}

fn oxide_commands(commands: Vec<Command>, plot: &mut Plot<'_>) -> Result {
    for command in commands {
        plot.record()?;
        let (x, y) = command.delta();
        match command {
            Command::Stitch { .. } => plot.relative(Move::Sew, x, -y),
            Command::Jump { .. } | Command::Trim { .. } => plot.relative(Move::Jump, x, -y),
            Command::ColorChange { .. } => {
                plot.relative(Move::Shift, x, -y);
                plot.change();
            }
            Command::End => break,
            _ => {}
        }
    }
    Ok(())
}
