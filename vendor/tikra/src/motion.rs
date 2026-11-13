//! The construction animation outside the browser, as GIF and APNG: each frame is a still SVG of
//! the moment, as the browser plays [`crate::Options::animated`]'s CSS, rasterized here. It takes
//! the feature `motion`, so the package for the browser, which makes its GIF on a canvas, does not
//! carry it.

use crate::{raster, Error};

/// Seconds the finished figure stays before the loop starts over.
const HOLD: f64 = 1.6;
/// The longest step, a stroke drawn along its length, as in the SVG's CSS.
const DRAW: f64 = 0.9;
/// The shorter one, what fades in.
const FADE: f64 = 0.45;
/// The stroke's share of [`DRAW`]: the fill follows.
const STROKE: f64 = 0.7;

/// How to make the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Motion {
    /// The width in pixels; the height follows the figure's, and both come out even.
    pub width: u32,
    pub fps: u32,
}

impl Default for Motion {
    fn default() -> Motion {
        Motion { width: 560, fps: 15 }
    }
}

/// CSS's `cubic-bezier(x1, 0, x2, 1)` at `t`, from 0 to 1.
fn ease(x1: f64, x2: f64, t: f64) -> f64 {
    if t <= 0.0 || t >= 1.0 {
        return t.clamp(0.0, 1.0);
    }
    let curve = |a: f64, b: f64, s: f64| 3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s;
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if curve(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(0.0, 1.0, (lo + hi) / 2.0)
}

/// `ease-in-out`.
fn ease_in_out(t: f64) -> f64 {
    ease(0.42, 0.58, t)
}

/// `ease-out`.
fn ease_out(t: f64) -> f64 {
    ease(0.0, 0.58, t)
}

/// How much of a stroke is drawn and how opaque its fill is, `t` seconds into its step.
fn stroke_at(t: f64) -> (f64, f64) {
    let p = (t / DRAW).clamp(0.0, 1.0);
    if p < STROKE {
        (ease_in_out(p / STROKE), 0.0)
    } else {
        (1.0, ease_in_out((p - STROKE) / (1.0 - STROKE)))
    }
}

/// How opaque what fades in is, `t` seconds into its step.
fn fade_at(t: f64) -> f64 {
    ease_out((t / FADE).clamp(0.0, 1.0))
}

/// The length of a path as tikra writes it (`M`, `L`, `C`, `Z`, absolute): `pathLength="1"` is not
/// read when rasterizing, so the dashes are in units of the figure.
fn path_length(d: &str) -> f64 {
    let (mut at, mut start, mut total) = ((0.0, 0.0), (0.0, 0.0), 0.0f64);
    let dist = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1);
    let mut words = d.split_ascii_whitespace().peekable();
    while let Some(word) = words.next() {
        let (kind, first) = word.split_at(1);
        let mut number = |first: Option<&str>| {
            first.filter(|s| !s.is_empty()).map(str::to_string).or_else(|| words.next().map(str::to_string)).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
        };
        match kind {
            "M" | "L" => {
                let p = (number(Some(first)), number(None));
                if kind == "M" {
                    start = p;
                } else {
                    total += dist(at, p);
                }
                at = p;
            }
            "C" => {
                let a = (number(Some(first)), number(None));
                let b = (number(None), number(None));
                let p = (number(None), number(None));
                let mut last = at;
                for k in 1..=32 {
                    let t = f64::from(k) / 32.0;
                    let u = 1.0 - t;
                    let of = |x0: f64, x1: f64, x2: f64, x3: f64| u * u * u * x0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x3;
                    let q = (of(at.0, a.0, b.0, p.0), of(at.1, a.1, b.1, p.1));
                    total += dist(last, q);
                    last = q;
                }
                at = p;
            }
            "Z" => {
                total += dist(at, start);
                at = start;
            }
            _ => {}
        }
    }
    total
}

/// The value of an attribute in a tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    Some(&tag[i..i + tag[i..].find('"')?])
}

/// A tag without an attribute.
fn without(tag: &str, name: &str) -> String {
    let key = format!(" {name}=\"");
    let Some(i) = tag.find(&key) else { return tag.to_string() };
    let value = i + key.len();
    let end = value + tag[value..].find('"').map_or(0, |e| e + 1);
    format!("{}{}", &tag[..i], &tag[end..])
}

/// The animated SVG as a still of the moment `t`, in seconds.
fn frame(svg: &str, t: f64, step: f64) -> String {
    const MARK: &str = " class=\"tr-";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(mark) = rest.find(MARK) {
        let Some(open) = rest[..mark].rfind('<') else { break };
        let close = mark + rest[mark..].find('>').unwrap_or(0);
        out.push_str(&rest[..open]);
        let whole = &rest[open..close];
        let tag = whole.strip_suffix('/').unwrap_or(whole);
        let number = attribute(tag, "style").and_then(|s| s.strip_prefix("--i:")).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let local = t - number * step;
        let drawn = tag.contains("class=\"tr-d\"");
        let mut bare = without(&without(&without(tag, "class"), "style"), "pathLength");
        if drawn {
            let length = attribute(tag, "d").map_or(0.0, path_length);
            let base = attribute(tag, "fill-opacity").and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
            let (line, fill) = stroke_at(local);
            bare = without(&bare, "fill-opacity");
            if line < 1.0 {
                bare.push_str(&format!(" stroke-dasharray=\"{} {}\" stroke-dashoffset=\"{}\"", length, 2.0 * length, 1.01 * length * (1.0 - line)));
            }
            bare.push_str(&format!(" fill-opacity=\"{}\"", base * fill));
        } else {
            let base = attribute(tag, "opacity").and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
            bare = without(&bare, "opacity");
            bare.push_str(&format!(" opacity=\"{}\"", base * fade_at(local)));
        }
        out.push_str(&bare);
        if tag.len() < whole.len() {
            out.push('/');
        }
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

/// The SVG without the animation's rules.
fn without_style(svg: &str) -> String {
    match (svg.find("<style>@keyframes tr-d"), svg.find("</style>")) {
        (Some(i), Some(j)) if i < j => format!("{}{}", &svg[..i], &svg[j + "</style>".len()..]),
        _ => svg.to_string(),
    }
}

/// The frames, RGBA on white, and each one's delay in hundredths of a second.
struct Frames {
    width: u32,
    height: u32,
    images: Vec<Vec<u8>>,
    delays: Vec<u16>,
}

fn frames(svg: &str, motion: Motion) -> Result<Frames, Error> {
    if motion.fps == 0 || motion.width == 0 {
        return Err(Error::Png("the width and the frames per second must be over 0".into()));
    }
    let steps = between(svg, "data-steps=\"", "\"").and_then(|s| s.parse::<f64>().ok());
    let step = between(svg, "calc(var(--i)*", "s)").and_then(|s| s.parse::<f64>().ok());
    let (Some(count), Some(step)) = (steps, step) else {
        return Err(Error::Png("not an animated SVG: convert with `Options::animated`".into()));
    };
    let total = (count - 1.0).max(0.0) * step + DRAW;
    let box_ = between(svg, "viewBox=\"", "\"").unwrap_or_default().split(' ').filter_map(|v| v.parse::<f64>().ok()).collect::<Vec<_>>();
    let ratio = if let [_, _, w, h] = box_[..] { h / w } else { 1.0 };
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(16);
    let (width, height) = (even(f64::from(motion.width)), even(f64::from(motion.width) * ratio));
    let last = ((total * f64::from(motion.fps)).ceil() as u32).max(2);
    let still = without_style(svg);
    let mut images = Vec::with_capacity(last as usize + 1);
    for k in 0..=last {
        images.push(raster(&frame(&still, f64::from(k) / f64::from(last) * total, step), width, height)?);
    }
    let mut delays = vec![(100.0 / f64::from(motion.fps)).round().max(2.0) as u16; images.len()];
    delays[last as usize] = (HOLD * 100.0) as u16;
    Ok(Frames { width, height, images, delays })
}

/// The text between two marks.
fn between<'a>(s: &'a str, from: &str, to: &str) -> Option<&'a str> {
    let i = s.find(from)? + from.len();
    Some(&s[i..i + s[i..].find(to)?])
}

/// The colors of the finished figure, the most frequent first: up to 256, the palette of every frame.
fn palette(pixels: &[u8]) -> Vec<[u8; 3]> {
    let mut counts = std::collections::BTreeMap::new();
    for p in pixels.as_chunks::<4>().0 {
        *counts.entry((u32::from(p[0]) >> 3) << 10 | (u32::from(p[1]) >> 3) << 5 | u32::from(p[2]) >> 3).or_insert(0u64) += 1;
    }
    let mut top: Vec<(u32, u64)> = counts.into_iter().collect();
    top.sort_by_key(|&(key, n)| (std::cmp::Reverse(n), key));
    // 5 bits back to 8, with 31 as 255: the white of the page stays white
    let full = |v: u32| (f64::from(v) * 255.0 / 31.0).round() as u8;
    let mut colors: Vec<[u8; 3]> = top.iter().take(256).map(|&(k, _)| [(k >> 10) & 31, (k >> 5) & 31, k & 31].map(full)).collect();
    colors.resize(256, [0, 0, 0]);
    colors
}

/// GIF's LZW for 8-bit indices: codes of 9 to 12 bits, least significant bit first.
fn lzw(indices: &[u8]) -> Vec<u8> {
    const CLEAR: u32 = 256;
    const END: u32 = 257;
    struct Bits {
        out: Vec<u8>,
        acc: u64,
        bits: u32,
        size: u32,
    }
    impl Bits {
        fn emit(&mut self, code: u32) {
            self.acc |= u64::from(code) << self.bits;
            self.bits += self.size;
            while self.bits >= 8 {
                self.out.push((self.acc & 255) as u8);
                self.acc >>= 8;
                self.bits -= 8;
            }
        }
    }
    let mut b = Bits { out: Vec::new(), acc: 0, bits: 0, size: 9 };
    let mut next = 258;
    let mut dict = std::collections::HashMap::new();
    b.emit(CLEAR);
    let mut prefix = u32::from(indices[0]);
    for &k in &indices[1..] {
        let key = (prefix << 8) | u32::from(k);
        if let Some(&known) = dict.get(&key) {
            prefix = known;
            continue;
        }
        b.emit(prefix);
        if next < 4096 {
            dict.insert(key, next);
            next += 1;
            if next > (1 << b.size) && b.size < 12 {
                b.size += 1;
            }
        } else {
            b.emit(CLEAR);
            dict.clear();
            next = 258;
            b.size = 9;
        }
        prefix = u32::from(k);
    }
    b.emit(prefix);
    b.emit(END);
    if b.bits > 0 {
        b.out.push((b.acc & 255) as u8);
    }
    b.out
}

/// The construction animation of an SVG from [`crate::Options::animated`] as GIF bytes, looping,
/// on white.
pub fn gif(svg: &str, motion: Motion) -> Result<Vec<u8>, Error> {
    encode_gif(&frames(svg, motion)?)
}

/// The frames of a pen drawing the figure of `svg`.
fn pen_frames(svg: &str, pen: &crate::pen::Pen, motion: Motion) -> Result<Frames, Error> {
    if motion.fps == 0 || motion.width == 0 {
        return Err(Error::Png("the width and the frames per second must be over 0".into()));
    }
    let (w, h) = crate::pen::sheet(svg, pen)?;
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(16);
    let (width, height) = (even(f64::from(motion.width)), even(f64::from(motion.width) * h / w));
    let last = ((pen.seconds * f64::from(motion.fps)).ceil() as usize).max(2);
    let mut images = Vec::with_capacity(last + 1);
    for still in crate::pen::stills(svg, pen, last)? {
        images.push(raster(&still, width, height)?);
    }
    let mut delays = vec![(100.0 / f64::from(motion.fps)).round().max(2.0) as u16; images.len()];
    delays[last] = (HOLD * 100.0) as u16;
    Ok(Frames { width, height, images, delays })
}

/// A pen drawing the figure of `svg`, the SVG tikra wrote for it, as GIF bytes.
pub fn pen_gif(svg: &str, pen: &crate::pen::Pen, motion: Motion) -> Result<Vec<u8>, Error> {
    encode_gif(&pen_frames(svg, pen, motion)?)
}

/// A pen drawing the figure of `svg`, the SVG tikra wrote for it, as APNG bytes.
pub fn pen_apng(svg: &str, pen: &crate::pen::Pen, motion: Motion) -> Result<Vec<u8>, Error> {
    encode_apng(&pen_frames(svg, pen, motion)?)
}

fn encode_gif(f: &Frames) -> Result<Vec<u8>, Error> {
    let colors = palette(f.images.last().map_or(&[][..], Vec::as_slice));
    let word = |v: u32| [(v & 255) as u8, (v >> 8) as u8];
    let mut out = b"GIF89a".to_vec();
    out.extend(word(f.width));
    out.extend(word(f.height));
    out.extend([0xf7, 0, 0]);
    out.extend(colors.iter().flatten());
    out.extend([0x21, 0xff, 11]);
    out.extend(b"NETSCAPE2.0");
    out.extend([3, 1, 0, 0, 0]);
    let mut nearest = std::collections::HashMap::new();
    let mut index = |r: u8, g: u8, b: u8| -> u8 {
        *nearest.entry((r, g, b)).or_insert_with(|| {
            let far = |c: &[u8; 3]| [(r, c[0]), (g, c[1]), (b, c[2])].iter().map(|&(a, b)| (i32::from(a) - i32::from(b)).pow(2)).sum::<i32>();
            colors.iter().enumerate().min_by_key(|(_, c)| far(c)).map_or(0, |(i, _)| i as u8)
        })
    };
    for (image, delay) in f.images.iter().zip(&f.delays) {
        let indices: Vec<u8> = image.as_chunks::<4>().0.iter().map(|p| index(p[0], p[1], p[2])).collect();
        out.extend([0x21, 0xf9, 4, 0]);
        out.extend(word(u32::from(*delay)));
        out.extend([0, 0, 0x2c, 0, 0, 0, 0]);
        out.extend(word(f.width));
        out.extend(word(f.height));
        out.extend([0, 8]);
        for block in lzw(&indices).chunks(255) {
            out.push(block.len() as u8);
            out.extend(block);
        }
        out.push(0);
    }
    out.push(0x3b);
    Ok(out)
}

/// The construction animation of an SVG from [`crate::Options::animated`] as APNG bytes, looping,
/// on white.
pub fn apng(svg: &str, motion: Motion) -> Result<Vec<u8>, Error> {
    encode_apng(&frames(svg, motion)?)
}

fn encode_apng(f: &Frames) -> Result<Vec<u8>, Error> {
    let failed = |e: png::EncodingError| Error::Png(e.to_string());
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, f.width, f.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(f.images.len() as u32, 0).map_err(failed)?;
    let mut writer = encoder.write_header().map_err(failed)?;
    for (image, delay) in f.images.iter().zip(&f.delays) {
        writer.set_frame_delay(*delay, 100).map_err(failed)?;
        writer.write_image_data(image).map_err(failed)?;
    }
    writer.finish().map_err(failed)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_easings_are_css() {
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(ease_out(1.0).round(), 1.0);
        // cubic-bezier(0,0,.58,1) at .5, as browsers compute it
        assert!((ease_out(0.5) - 0.6846).abs() < 1e-3, "{}", ease_out(0.5));
    }

    #[test]
    fn a_stroke_is_drawn_and_then_filled() {
        assert_eq!(stroke_at(-1.0), (0.0, 0.0));
        assert_eq!(stroke_at(DRAW * STROKE), (1.0, 0.0));
        let (line, fill) = stroke_at(DRAW);
        assert_eq!(line, 1.0);
        assert!((fill - 1.0).abs() < 1e-6);
    }

    #[test]
    fn path_lengths() {
        assert!((path_length("M0 0 L3 4") - 5.0).abs() < 1e-9);
        assert!((path_length("M0 0 L3 0 L3 4 Z") - 12.0).abs() < 1e-9);
        assert!((path_length("M0 0 M10 10 L10 11") - 1.0).abs() < 1e-9);
        assert!((path_length("M0 0 C1 0 2 0 3 0") - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_tag_of_a_frame() {
        let tag = r##"<path d="M0 0 L3 4" stroke="#000" data-step="0" class="tr-d" pathLength="1" style="--i:0"/>"##;
        let at_start = frame(tag, 0.0, 0.1);
        assert!(at_start.contains(r#"stroke-dashoffset="5.05""#) && at_start.contains(r#"fill-opacity="0""#), "{at_start}");
        assert!(!at_start.contains("class") && !at_start.contains("pathLength"));
        assert!(!frame(tag, 5.0, 0.1).contains("stroke-dasharray"));
        let fading = r#"<g opacity="0.5" class="tr-f" style="--i:2">"#;
        assert!(frame(fading, 0.0, 0.1).contains(r#" opacity="0""#));
        assert!(frame(fading, 5.0, 0.1).contains(r#" opacity="0.5""#) || frame(fading, 5.0, 0.1).contains(r#" opacity="0.49"#));
    }
}
