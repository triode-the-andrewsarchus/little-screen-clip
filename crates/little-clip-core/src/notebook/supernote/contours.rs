//! Authored Supernote TOTALPATH outlines. The pinned supernote-typescript
//! vector-format specification documents record framing, contours and status.
use super::super::{check_cancel, vector};
use image::RgbImage;
use resvg::tiny_skia::{Pixmap, PremultipliedColorU8, Transform};
use std::{fmt::Write, sync::atomic::AtomicBool};

const MAX_TRAILS: u32 = 100_000;
const MAX_SVG: usize = 16 * 1024 * 1024;
const CONFIG_SIZE: usize = 208;

pub(super) fn decode(data: &[u8], highres_palette: bool) -> Result<[Option<String>; 4], String> {
    let mut block = TrailBytes(data);
    let count = block.u32()?;
    if count > MAX_TRAILS {
        return Err("Supernote stroke count exceeds the detail budget".into());
    }
    let mut layers = std::array::from_fn(|_| Some(String::new()));
    for _ in 0..count {
        let record = block.counted(1)?;
        let (config, payload) = record
            .split_at_checked(CONFIG_SIZE)
            .ok_or("Truncated Supernote stroke header")?;
        let layer = word(config, 44)? as usize;
        let paths = layers
            .get_mut(layer)
            .ok_or("Unknown Supernote stroke layer")?;
        if let Some(svg) = paths
            && append_trail(svg, config, TrailBytes(payload), highres_palette).is_err()
        {
            // Record framing and layer identity are known. A stroke failure
            // here only invalidates this layer's retained representation.
            *paths = None;
        }
    }
    if !block.0.is_empty() {
        return Err("Unexpected data after Supernote stroke records".into());
    }
    Ok(layers)
}

fn append_trail(
    svg: &mut String,
    config: &[u8],
    trail: TrailBytes<'_>,
    highres_palette: bool,
) -> Result<(), String> {
    let class = word(config, 40)? as i32;
    let color = word(config, 4)?;
    let label = &config[48..100];
    let label = label.split(|byte| *byte == 0).next().unwrap_or_default();
    if matches!(class, -1 | -2 | -4 | -5) || color == 255 || label == b"0000" {
        return Ok(());
    }
    let Some(contour) = live_contour(trail)? else {
        return Ok(());
    };
    let pen = word(config, 0)?;
    if !matches!(pen, 1 | 5 | 10 | 11 | 15 | 16)
        || !matches!((class, label), (5000, b"others") | (0, b"straightLine"))
    {
        return Err("Unsupported Supernote stroke type".into());
    }
    let gray = ink_gray(color, highres_palette).ok_or("Unsupported Supernote stroke color")?;
    let style = if matches!(pen, 5 | 11) && gray != 254 {
        " style='mix-blend-mode:darken'"
    } else {
        ""
    };
    let _ = write!(
        svg,
        "<path fill='#{gray:02x}{gray:02x}{gray:02x}'{style} d='"
    );
    append_contour(svg, contour)
}

fn live_contour(mut trail: TrailBytes<'_>) -> Result<Option<TrailBytes<'_>>, String> {
    for stride in [24, 8, 2, 4, 1, 8, 4] {
        trail.counted(stride)?;
    }
    match word(trail.take(52)?, 0)? as i32 {
        0 => {
            trail.counted(4)?;
            trail.take(10)?;
            Ok(Some(trail))
        }
        // Partial erases leave replacement contours as their own live records.
        -99 | -16 | -4 | -3 | -2 => Ok(None),
        _ => Err("Unsupported Supernote stroke status".into()),
    }
}

fn append_contour(svg: &mut String, mut trail: TrailBytes<'_>) -> Result<(), String> {
    let rings = trail.u32()?;
    if !(1..=1000).contains(&rings) {
        return Err("Invalid Supernote contour count".into());
    }
    for _ in 0..rings {
        let points = trail.counted(8)?;
        if points.len() < 24 {
            return Err("Invalid Supernote contour length".into());
        }
        for (index, point) in points.chunks_exact(8).enumerate() {
            let x = f32::from_bits(word(point, 0)?);
            let y = f32::from_bits(word(point, 4)?);
            if ![x, y]
                .into_iter()
                .all(|xy| xy.is_finite() && xy.abs() <= 65_536.0)
            {
                return Err("Invalid Supernote contour coordinate".into());
            }
            let command = if index == 0 { 'M' } else { 'L' };
            let _ = write!(svg, "{command}{x:.3} {y:.3}");
            if svg.len() > MAX_SVG {
                return Err("Supernote contours exceed the detail budget".into());
            }
        }
        svg.push('Z');
    }
    svg.push_str("'/>");
    Ok(())
}

fn ink_gray(color: u32, highres: bool) -> Option<u8> {
    Some(match color {
        0 | 1 => 0,
        48 | 49 if highres => 48,
        80 | 81 if highres => 80,
        48 | 49 | 157 | 158 => 157,
        80 | 81 | 201 | 202 => 201,
        254 => 254,
        _ => return None,
    })
}

struct TrailBytes<'a>(&'a [u8]);

impl<'a> TrailBytes<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let (head, tail) = self
            .0
            .split_at_checked(len)
            .ok_or("Truncated Supernote stroke")?;
        self.0 = tail;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, String> {
        word(self.take(4)?, 0)
    }

    fn counted(&mut self, stride: usize) -> Result<&'a [u8], String> {
        let count = self.u32()? as usize;
        self.take(
            count
                .checked_mul(stride)
                .ok_or("Supernote stroke array overflow")?,
        )
    }
}

fn word(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = bytes
        .get(offset..offset + 4)
        .ok_or("Truncated Supernote stroke field")?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(super) fn qualifies(
    group: &str,
    saved: &RgbImage,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let (width, height) = saved.dimensions();
    let svg = vector::page_svg(width, height) + group + "</svg>";
    let tree = vector::parse(&svg)?;
    let mut rendered = Pixmap::new(width, height).ok_or("Supernote contour allocation failed")?;
    resvg::render(&tree, Transform::identity(), &mut rendered.as_mut());
    // Compare strongest nearby ink in both directions. Saved raster edges can
    // be quantized toward paper white, and both renderers can shift edges by a
    // pixel or two. Unexplained ink makes the saved layer authoritative.
    for y in 0..height {
        check_cancel(cancel)?;
        for x in 0..width {
            let rgb @ [red, green, blue] = saved.get_pixel(x, y).0;
            if red != green || red != blue {
                return Ok(false);
            }
            let a = saved_ink(rgb);
            let b = contour_ink(rendered.pixel(x, y));
            if a.is_some_and(|gray| {
                !near(x, y, width, height, gray, |x, y| {
                    contour_ink(rendered.pixel(x, y))
                })
            }) || b.is_some_and(|gray| {
                !near(x, y, width, height, gray, |x, y| {
                    saved_ink(saved.get_pixel(x, y).0)
                })
            }) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn saved_ink(rgb: [u8; 3]) -> Option<u8> {
    (rgb != [255; 3]).then_some(rgb[0])
}

fn contour_ink(color: Option<PremultipliedColorU8>) -> Option<u8> {
    let color = color.filter(|color| color.alpha() >= 128)?;
    Some(color.demultiply().red())
}

fn near(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    gray: u8,
    sample: impl Fn(u32, u32) -> Option<u8>,
) -> bool {
    let matches = |x, y| match sample(x, y) {
        Some(value) if value == 254 || gray == 254 => value == gray,
        Some(value) => value <= gray.saturating_add(20),
        None => false,
    };
    matches(x, y)
        || (y.saturating_sub(2)..=(y + 2).min(height - 1))
            .any(|y| (x.saturating_sub(2)..=(x + 2).min(width - 1)).any(|x| matches(x, y)))
}
