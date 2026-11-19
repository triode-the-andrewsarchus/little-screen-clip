use super::{
    BrushDynamics, BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind,
    MAX_BRUSH_LABEL_BYTES, ParametricShape, bounded_label, mask,
};

use std::sync::Arc;

const MAX_GIMP_DIM: u32 = 4096;

pub(super) fn parse_gbr_library(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    let (preset, _) = parse_gbr_at(bytes, fallback_name)?;
    Ok(BrushLibrary {
        format: BrushFormat::GimpGbr,
        brushes: vec![preset],
        total_count: 1,
    })
}

pub(super) fn parse_gbr_at(
    bytes: &[u8],
    fallback_name: &str,
) -> Result<(BrushPreset, usize), String> {
    let header = BitmapHeader::read(bytes)?;
    let BitmapHeader {
        width,
        height,
        name_offset,
        data_offset,
        bpp,
        spacing,
    } = header;
    let name_bytes = &bytes[name_offset..data_offset.min(name_offset + MAX_BRUSH_LABEL_BYTES)];
    let name_end = name_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(name_bytes.len());
    let parsed_name = String::from_utf8_lossy(&name_bytes[..name_end])
        .trim()
        .to_owned();
    let name = if parsed_name.is_empty() {
        fallback_name.to_owned()
    } else {
        parsed_name
    };

    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("Overflow")?;
    let payload_len = pixel_count.checked_mul(bpp).ok_or("Overflow")?;
    let end = data_offset.checked_add(payload_len).ok_or("Overflow")?;
    let payload = bytes
        .get(data_offset..end)
        .ok_or("Truncated .gbr pixel payload")?;
    let alpha = match bpp {
        1 => payload.to_vec(),
        2 => payload
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pixel| {
                // CinePaint stores only the most significant half of an f32.
                let value = f32::from_bits(u32::from(u16::from_be_bytes(pixel)) << 16);
                (value.clamp(0.0, 1.0) * 255.0).round() as u8
            })
            .collect(),
        _ => payload
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel[3])
            .collect(),
    };

    let tip = mask::raster(width, height, alpha).ok_or("Invalid .gbr mask")?;
    let detail = format!(
        "{width}×{height} · {}% spacing",
        (spacing * 100.0).round() as u32
    );
    Ok((
        BrushPreset {
            name,
            tip,
            params: BrushStrokeParams {
                size_px: width.max(height) as f32,
                spacing,
                dynamics: BrushDynamics {
                    axis_spacing: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            detail,
        },
        end,
    ))
}

struct BitmapHeader {
    width: u32,
    height: u32,
    name_offset: usize,
    data_offset: usize,
    bpp: usize,
    spacing: f32,
}

impl BitmapHeader {
    fn read(bytes: &[u8]) -> Result<Self, String> {
        let fixed = bytes
            .first_chunk::<20>()
            .ok_or("Truncated GIMP .gbr header")?;
        let words = fixed.as_chunks::<4>().0;
        let [data_offset, version, width, height, depth] =
            std::array::from_fn(|index| u32::from_be_bytes(words[index]));
        let (name_offset, spacing) = match version {
            1 => (20, 25),
            2 | 3 if bytes.get(20..24) == Some(b"GIMP") => {
                let spacing = bytes
                    .get(24..28)
                    .and_then(|raw| raw.try_into().ok())
                    .map(u32::from_be_bytes)
                    .ok_or("Truncated GIMP .gbr spacing")?;
                (28, spacing)
            }
            _ => return Err("Unsupported GIMP .gbr version or magic".into()),
        };
        let data_offset = data_offset as usize;
        let bpp = match (version, depth) {
            (1 | 2, 1 | 4) => depth as usize,
            (3, 18) => 2,
            _ => return Err(format!("Unsupported .gbr depth: {depth}")),
        };
        if !(name_offset..=bytes.len()).contains(&data_offset) {
            return Err("Invalid .gbr header size".into());
        }
        if !(1..=MAX_GIMP_DIM).contains(&width) || !(1..=MAX_GIMP_DIM).contains(&height) {
            return Err("Invalid .gbr brush dimensions".into());
        }
        Ok(Self {
            width,
            height,
            name_offset,
            data_offset,
            bpp,
            spacing: spacing.clamp(1, 500) as f32 / 100.0,
        })
    }
}

pub(super) fn parse_gih(
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
) -> Result<BrushLibrary, String> {
    const MAX_HEADER_LINE: usize = 8 * 1024;
    let first_nl = bytes[..bytes.len().min(MAX_HEADER_LINE)]
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("Invalid .gih header")?;
    let rest = &bytes[first_nl + 1..];
    let second_nl = rest[..rest.len().min(MAX_HEADER_LINE)]
        .iter()
        .position(|&b| b == b'\n')
        .map(|pos| first_nl + 1 + pos)
        .ok_or("Invalid .gih parameter line")?;

    let base_name = String::from_utf8_lossy(&bytes[..first_nl.min(MAX_BRUSH_LABEL_BYTES)])
        .trim()
        .to_owned();
    let header_line = String::from_utf8_lossy(&bytes[first_nl + 1..second_nl]);
    let ncells = header_line
        .split_whitespace()
        .next()
        .and_then(|tok| tok.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 512);

    let mut offset = second_nl + 1;
    let mut brushes = Vec::new();
    for idx in 0..ncells.min(max_brushes) {
        if offset >= bytes.len() {
            break;
        }
        let cell_name = if base_name.is_empty() {
            format!("{fallback_name} #{}", idx + 1)
        } else {
            format!("{base_name} #{}", idx + 1)
        };
        let Ok((preset, consumed)) = parse_gbr_at(&bytes[offset..], &cell_name) else {
            break;
        };
        brushes.push(preset);
        offset += consumed;
    }

    if brushes.is_empty() {
        return Err("No valid cells in GIMP .gih hose".into());
    }
    if brushes.len() == ncells
        && let Some(axes) = hose_axes(&header_line, ncells)
    {
        let hose = Arc::new(ImageHose {
            tips: brushes.iter().map(|brush| brush.tip.clone()).collect(),
            axes,
        });
        for (phase, brush) in brushes.iter_mut().enumerate() {
            brush.params.dynamics.tip_sequence = Some(TipSequence {
                hose: hose.clone(),
                phase,
            });
        }
    }
    Ok(BrushLibrary {
        format: BrushFormat::GimpGih,
        brushes,
        total_count: ncells,
    })
}

#[derive(Clone, Copy, Debug)]
enum HoseSelection {
    Constant,
    Incremental,
    Pressure,
    Random,
    Angular,
    Velocity,
    Tilt,
}

#[derive(Debug)]
struct HoseAxis {
    rank: usize,
    select: HoseSelection,
}

#[derive(Debug)]
struct ImageHose {
    tips: Vec<BrushTipKind>,
    axes: Vec<HoseAxis>,
}

#[derive(Clone, Debug)]
pub(super) struct TipSequence {
    hose: Arc<ImageHose>,
    phase: usize,
}

impl TipSequence {
    pub(super) fn tip_for_dab(
        &self,
        dab: usize,
        pressure: f32,
        direction: f32,
        seed: u64,
    ) -> &BrushTipKind {
        let mut index = 0;
        let mut stride = 1;
        for (dimension, axis) in self.hose.axes.iter().rev().enumerate() {
            let phase = self.phase / stride;
            let random = (seed >> (16 * dimension)) & 0xffff;
            let coordinate = axis.position(dab, phase, pressure, direction, random);
            index += stride * coordinate;
            stride *= axis.rank;
        }
        &self.hose.tips[index.min(self.hose.tips.len() - 1)]
    }
}

impl HoseAxis {
    fn position(
        &self,
        dab: usize,
        phase: usize,
        pressure: f32,
        direction: f32,
        random: u64,
    ) -> usize {
        let rank = self.rank as f32;
        match self.select {
            HoseSelection::Constant => 0,
            HoseSelection::Incremental => (phase + dab) % self.rank,
            HoseSelection::Pressure => (pressure * (rank - 1.0)).round_ties_even() as usize,
            HoseSelection::Random => random as usize % self.rank,
            // GIMP's angular selector points its first sector upward in screen coordinates.
            HoseSelection::Angular => ((direction / std::f32::consts::TAU + 0.25) * rank)
                .round_ties_even()
                .rem_euclid(rank) as usize,
            // The sample has a steady moderate speed and an upright pen.
            HoseSelection::Velocity => (0.5 * rank).round() as usize,
            HoseSelection::Tilt => self.rank / 2,
        }
        .min(self.rank - 1)
    }
}

fn hose_axes(header: &str, available: usize) -> Option<Vec<HoseAxis>> {
    if header.split_ascii_whitespace().count() == 1 {
        return Some(vec![HoseAxis {
            rank: available,
            select: HoseSelection::Incremental,
        }]);
    }
    let field = |key: &str| {
        header
            .split_ascii_whitespace()
            .filter_map(|token| token.split_once(':'))
            .find_map(|(name, value)| (name == key).then_some(value))
    };
    let dimensions = field("dim")?.parse::<usize>().ok()?;
    if !(1..=4).contains(&dimensions) {
        return None;
    }
    let axes: Vec<_> = (0..dimensions)
        .map(|dimension| {
            let rank = field(&format!("rank{dimension}"))?.parse::<usize>().ok()?;
            if !(1..=512).contains(&rank) {
                return None;
            }
            let select = match field(&format!("sel{dimension}")) {
                Some("incremental") => HoseSelection::Incremental,
                Some("pressure") => HoseSelection::Pressure,
                Some("random") => HoseSelection::Random,
                Some("angular") => HoseSelection::Angular,
                Some("velocity") => HoseSelection::Velocity,
                Some("xtilt" | "ytilt") => HoseSelection::Tilt,
                _ => HoseSelection::Constant,
            };
            Some(HoseAxis { rank, select })
        })
        .collect::<Option<_>>()?;
    axes.iter()
        .try_fold(1_usize, |cells, axis| cells.checked_mul(axis.rank))?;
    Some(axes)
}

pub(super) fn parse_vbr(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    Ok(BrushLibrary {
        format: BrushFormat::GimpVbr,
        brushes: vec![parse_vbr_preset(bytes, fallback_name)?],
        total_count: 1,
    })
}

pub(super) fn parse_vbr_preset(bytes: &[u8], fallback_name: &str) -> Result<BrushPreset, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 in .vbr brush")?;
    let mut lines = text.lines().map(str::trim);
    if !lines
        .next()
        .is_some_and(|line| line.starts_with("GIMP-VBR"))
    {
        return Err("Invalid .vbr magic header".into());
    }
    let has_shape = match lines.next() {
        Some(version) if version.starts_with("1.0") => false,
        Some(version) if version.starts_with("1.5") => true,
        _ => return Err("Unsupported .vbr version".into()),
    };
    let name = lines
        .next()
        .filter(|name| !name.is_empty())
        .map(bounded_label)
        .unwrap_or(fallback_name)
        .to_owned();
    let shape = if has_shape {
        match lines.next() {
            Some("circle") => ParametricShape::Circle,
            Some("square") => ParametricShape::Square,
            Some("diamond") => ParametricShape::Diamond,
            _ => return Err("Invalid .vbr brush shape".into()),
        }
    } else {
        ParametricShape::Circle
    };
    let spacing = vbr_number(lines.next(), "spacing")?.clamp(1.0, 500.0) / 100.0;
    let radius = vbr_number(lines.next(), "radius")?.clamp(0.5, 2000.0);
    let spikes = if has_shape {
        lines
            .next()
            .and_then(|value| value.parse::<u8>().ok())
            .ok_or("Invalid .vbr spike count")?
            .clamp(2, 20)
    } else {
        2
    };
    let hardness = vbr_number(lines.next(), "hardness")?.clamp(0.0, 1.0);
    let aspect_ratio = vbr_number(lines.next(), "aspect ratio")?.clamp(1.0, 20.0);
    let angle_deg = vbr_number(lines.next(), "angle")?;

    let detail = format!("Ø {:.0}px · hardness {:.2}", radius * 2.0, hardness);
    Ok(BrushPreset {
        name,
        tip: generated_vbr_tip(shape, hardness, aspect_ratio, spikes),
        params: BrushStrokeParams {
            size_px: radius * 2.0,
            spacing,
            angle_deg: -angle_deg,
            hardness,
            dynamics: BrushDynamics {
                axis_spacing: true,
                ..Default::default()
            },
            ..Default::default()
        },
        detail,
    })
}

fn generated_vbr_tip(
    shape: ParametricShape,
    hardness: f32,
    aspect: f32,
    spikes: u8,
) -> BrushTipKind {
    let short_axis = aspect.recip();
    let (extent, ratio) = match (spikes > 2, shape) {
        (false, _) => (1.0, short_axis),
        (true, ParametricShape::Square) => (1.0_f32.hypot(short_axis), 1.0),
        (true, _) => (1.0, 1.0),
    };
    let exponent = 0.4 / (1.0 - hardness).max(0.000_000_4);
    mask::generated(ratio, |x, y| {
        let (x, y) = mask::fold_spikes(x * extent, y * extent * ratio, spikes);
        let distance = mask::shape_distance(shape, x, y * aspect);
        let falloff = distance.min(1.0).powf(exponent);
        if falloff <= 0.5 {
            1.0 - 2.0 * falloff * falloff
        } else {
            2.0 * (1.0 - falloff).powi(2)
        }
    })
}

fn vbr_number(value: Option<&str>, name: &str) -> Result<f32, String> {
    value
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("Invalid .vbr brush {name}"))
}

#[cfg(test)]
mod generated_tests {
    use super::*;

    #[test]
    fn bitmap_tips_accept_original_and_cinepaint_headers() {
        let legacy: Vec<u8> = [22_u32, 1, 2, 1, 1]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .chain([b'A', 0, 127, 255])
            .collect();
        let (tip, bytes) = parse_gbr_at(&legacy, "fallback").unwrap();
        assert_eq!(tip.name, "A");
        assert_eq!(tip.params.spacing, 0.25);
        assert_eq!(bytes, legacy.len());
        assert!(matches!(tip.tip, BrushTipKind::AlphaMask { alpha, .. } if *alpha == [127, 255]));

        let float: Vec<u8> = [28_u32, 3, 2, 1, 18, u32::from_be_bytes(*b"GIMP"), 60]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .chain([0x3f, 0, 0x3f, 0x80])
            .collect();
        let (tip, bytes) = parse_gbr_at(&float, "fallback").unwrap();
        assert_eq!(tip.params.spacing, 0.6);
        assert_eq!(bytes, float.len());
        assert!(matches!(tip.tip, BrushTipKind::AlphaMask { alpha, .. } if *alpha == [128, 255]));
    }

    #[test]
    fn image_hose_combines_pressure_and_angular_axes() {
        let axes = hose_axes("8 dim:2 rank0:2 sel0:pressure rank1:4 sel1:angular", 8).unwrap();
        let tips = (0..8)
            .map(|index| BrushTipKind::AlphaMask {
                width: 1,
                height: 1,
                alpha: [index].into(),
            })
            .collect();
        let hose = TipSequence {
            hose: Arc::new(ImageHose { tips, axes }),
            phase: 0,
        };
        let choose = |pressure, direction| {
            let BrushTipKind::AlphaMask { alpha, .. } = hose.tip_for_dab(0, pressure, direction, 0)
            else {
                panic!("expected raster tip")
            };
            alpha[0]
        };
        assert_eq!(choose(0.0, -std::f32::consts::FRAC_PI_2), 0);
        assert_eq!(choose(1.0, 0.0), 5);
        assert_eq!(choose(1.0, std::f32::consts::FRAC_PI_2), 6);
    }

    #[test]
    fn vbr_hardness_is_a_radial_falloff_and_spikes_repeat_the_tip_profile() {
        let BrushTipKind::AlphaMask {
            width,
            height,
            alpha,
        } = generated_vbr_tip(ParametricShape::Circle, 0.0, 1.0, 2)
        else {
            panic!("expected raster tip")
        };
        let y = (height / 2 * width) as usize;
        assert_eq!(alpha[y + (width / 2) as usize], 255);
        assert!((20..=45).contains(&alpha[y + (width * 3 / 4) as usize]));
        let plain = generated_vbr_tip(ParametricShape::Circle, 0.8, 1.0, 2);
        let spiked = generated_vbr_tip(ParametricShape::Circle, 0.8, 1.0, 6);
        let (
            BrushTipKind::AlphaMask { alpha: plain, .. },
            BrushTipKind::AlphaMask { alpha: spiked, .. },
        ) = (plain, spiked)
        else {
            panic!("expected raster tips")
        };
        assert!(
            plain
                .iter()
                .zip(spiked.iter())
                .all(|(a, b)| a.abs_diff(*b) <= 1)
        );
    }
}
