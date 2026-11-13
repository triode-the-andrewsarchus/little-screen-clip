//! Times: labels in `\fontfamily{ptm}` are laid out by mathra in STIX Two Math (its letters are a
//! Times), then each letter is drawn in TeX Gyre Termes, the Times of TeX Live, at Termes' widths
//! (those TeX's `ptm` sets text with) and kerned as TeX kerns it. Height and depth are those of
//! Termes' ink, as TeX's metrics for these fonts are.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::OnceLock;

use mathra::layout::{Item, MathBox};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

use crate::label::TIMES;

static FONTS: [&[u8]; 4] = [
    include_bytes!("fonts/texgyretermes-regular-subset.otf"),
    include_bytes!("fonts/texgyretermes-bold-subset.otf"),
    include_bytes!("fonts/texgyretermes-italic-subset.otf"),
    include_bytes!("fonts/texgyretermes-bolditalic-subset.otf"),
];

/// Glyph ids of Termes, as a label carries them: from `FIRST` on, the style in the next two bits
/// and the glyph of that style's font in the low twelve (STIX Two's own ids stay below).
const FIRST: u16 = 0xC000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Regular = 0,
    Bold = 1,
    Italic = 2,
    BoldItalic = 3,
}

fn face(style: usize) -> &'static Face<'static> {
    static FACES: OnceLock<Vec<Face<'static>>> = OnceLock::new();
    &FACES.get_or_init(|| FONTS.iter().map(|f| Face::parse(f, 0).expect("the embedded Termes parses")).collect())[style]
}

/// Whether a label's glyph is one of Termes.
pub fn is_termes(id: GlyphId) -> bool {
    id.0 >= FIRST
}

/// A Termes glyph's outline as the `d` of an SVG `<path>`, y up, as mathra writes its own.
pub fn outline(id: GlyphId) -> String {
    let style = usize::from((id.0 - FIRST) >> 12);
    let mut path = PathWriter(String::new());
    face(style).outline_glyph(GlyphId(id.0 & 0x0fff), &mut path);
    path.0.trim_end().to_string()
}

/// What STIX Two's glyphs stand for: a character in a style.
fn characters() -> &'static HashMap<u16, (char, Style)> {
    static MAP: OnceLock<HashMap<u16, (char, Style)>> = OnceLock::new();
    MAP.get_or_init(|| {
        let stix = TIMES.get();
        let mut m = HashMap::new();
        let mut add = |c: Option<char>, text: char, style: Style| {
            if let Some(g) = c.and_then(|c| stix.glyph(c)) {
                m.entry(g.0).or_insert((text, style));
            }
        };
        // the alphabets of math that text in a style is set with
        for (k, c) in ('A'..='Z').enumerate() {
            let k = k as u32;
            add(char::from_u32(0x1D400 + k), c, Style::Bold);
            add(char::from_u32(0x1D434 + k), c, Style::Italic);
            add(char::from_u32(0x1D468 + k), c, Style::BoldItalic);
        }
        for (k, c) in ('a'..='z').enumerate() {
            let k = k as u32;
            add(char::from_u32(0x1D41A + k), c, Style::Bold);
            // the italic h is Planck's constant, out of the block
            add(if c == 'h' { Some('\u{210E}') } else { char::from_u32(0x1D44E + k) }, c, Style::Italic);
            add(char::from_u32(0x1D482 + k), c, Style::BoldItalic);
        }
        for (k, c) in ('0'..='9').enumerate() {
            add(char::from_u32(0x1D7CE + k as u32), c, Style::Bold);
        }
        // and what text keeps as it is
        for c in (' '..='~').chain('\u{A0}'..='\u{FF}').chain(['\u{2013}', '\u{2014}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{FB01}', '\u{FB02}']) {
            add(Some(c), c, Style::Regular);
        }
        m
    })
}

/// Kerning of TeX's `ptm` (T1), in font units, from TeX's own widths: for each pair of letters,
/// digits and `.,;:!?()-/[]*`, how much narrower or wider TeX sets it than its two characters
/// apart. In the order of the fonts: roman, bold, italic, bold italic.
#[rustfmt::skip]
static KERNS: [&[(&str, i16)]; 4] = [
    // roman
    &[
        ("AC", -40), ("AG", -40), ("AO", -55), ("AQ", -55), ("AT", -111), ("AU", -55), ("AV", -135),
        ("AW", -90), ("AY", -105), ("Av", -74), ("Aw", -92), ("Ay", -92), ("BA", -35), ("BU", -10),
        ("DA", -40), ("DV", -40), ("DW", -30), ("DY", -55), ("F,", -80), ("F.", -80), ("FA", -74),
        ("Fa", -15), ("Fo", -15), ("JA", -60), ("KO", -30), ("Ke", -25), ("Ko", -35), ("Ku", -15),
        ("Ky", -25), ("LT", -92), ("LV", -100), ("LW", -74), ("LY", -100), ("Ly", -55), ("NA", -35),
        ("OA", -35), ("OT", -40), ("OV", -50), ("OW", -35), ("OX", -40), ("OY", -50), ("P,", -111),
        ("P.", -111), ("PA", -92), ("Pa", -15), ("QU", -10), ("RO", -40), ("RT", -60), ("RU", -40),
        ("RV", -80), ("RW", -55), ("RY", -65), ("T,", -74), ("T-", -92), ("T.", -74), ("T:", -50),
        ("T;", -55), ("TA", -93), ("TO", -18), ("Ta", -80), ("Te", -70), ("Ti", -35), ("To", -80),
        ("Tr", -35), ("Tu", -45), ("Tw", -80), ("Ty", -80), ("UA", -40), ("V,", -129), ("V-", -100),
        ("V.", -129), ("V:", -74), ("V;", -74), ("VA", -135), ("VG", -15), ("VO", -40),
        ("Va", -111), ("Ve", -111), ("Vi", -60), ("Vo", -129), ("Vu", -75), ("W,", -92),
        ("W-", -65), ("W.", -92), ("W:", -37), ("W;", -37), ("WA", -120), ("WO", -10), ("Wa", -80),
        ("We", -80), ("Wi", -40), ("Wo", -80), ("Wu", -50), ("Wy", -73), ("Y,", -129), ("Y-", -111),
        ("Y.", -129), ("Y:", -92), ("Y;", -92), ("YA", -120), ("YO", -30), ("Ya", -100),
        ("Ye", -100), ("Yi", -55), ("Yo", -110), ("Yu", -111), ("av", -20), ("aw", -15),
        ("b.", -40), ("bu", -20), ("bv", -15), ("cy", -15), ("eg", -15), ("ev", -25), ("ew", -25),
        ("ex", -15), ("ey", -15), ("fa", -10), ("ga", -5), ("hy", -5), ("iv", -25), ("ke", -10),
        ("ko", -10), ("ky", -15), ("lw", -10), ("nv", -40), ("ny", -15), ("ov", -15), ("ow", -25),
        ("oy", -10), ("py", -10), ("r,", -40), ("r-", -20), ("r.", -55), ("rg", -18), ("v,", -65),
        ("v.", -65), ("va", -25), ("ve", -15), ("vo", -20), ("w,", -65), ("w.", -65), ("wa", -10),
        ("wo", -10), ("xe", -15), ("y,", -65), ("y.", -65),
    ],
    // bold
    &[
        ("AC", -55), ("AG", -55), ("AO", -45), ("AQ", -45), ("AT", -95), ("AU", -50), ("AV", -145),
        ("AW", -130), ("AY", -100), ("Ap", -25), ("Au", -50), ("Av", -100), ("Aw", -90),
        ("Ay", -74), ("BA", -30), ("BU", -10), ("D.", -20), ("DA", -35), ("DV", -40), ("DW", -40),
        ("DY", -40), ("F,", -92), ("F.", -110), ("FA", -90), ("Fa", -25), ("Fe", -25), ("Fo", -25),
        ("J.", -20), ("JA", -30), ("Ja", -15), ("Je", -15), ("Jo", -15), ("Ju", -15), ("KO", -30),
        ("Ke", -25), ("Ko", -25), ("Ku", -15), ("Ky", -45), ("LT", -92), ("LV", -92), ("LW", -92),
        ("LY", -92), ("Ly", -55), ("NA", -20), ("OA", -40), ("OT", -40), ("OV", -50), ("OW", -50),
        ("OX", -40), ("OY", -50), ("P,", -92), ("P.", -110), ("PA", -74), ("Pa", -10), ("Pe", -20),
        ("Po", -20), ("Q.", -20), ("QU", -10), ("RO", -30), ("RT", -40), ("RU", -30), ("RV", -55),
        ("RW", -35), ("RY", -35), ("T,", -74), ("T-", -92), ("T.", -90), ("T:", -74), ("T;", -74),
        ("TA", -90), ("TO", -18), ("Ta", -92), ("Te", -92), ("Ti", -18), ("To", -92), ("Tr", -74),
        ("Tu", -92), ("Tw", -74), ("Ty", -74), ("U,", -50), ("U.", -50), ("UA", -60), ("V,", -129),
        ("V-", -74), ("V.", -145), ("V:", -92), ("V;", -92), ("VA", -135), ("VG", -30), ("VO", -45),
        ("Va", -92), ("Ve", -100), ("Vi", -37), ("Vo", -100), ("Vu", -92), ("W,", -92), ("W-", -37),
        ("W.", -92), ("W:", -55), ("W;", -55), ("WA", -120), ("WO", -10), ("Wa", -65), ("We", -65),
        ("Wi", -18), ("Wo", -75), ("Wu", -50), ("Wy", -60), ("Y,", -92), ("Y-", -92), ("Y.", -92),
        ("Y:", -92), ("Y;", -92), ("YA", -110), ("YO", -35), ("Ya", -85), ("Ye", -111), ("Yi", -37),
        ("Yo", -111), ("Yu", -92), ("av", -25), ("b.", -40), ("bb", -10), ("bu", -20), ("bv", -15),
        ("dw", -15), ("ev", -15), ("f,", -15), ("f.", -15), ("fo", -25), ("g.", -15), ("hy", -15),
        ("iv", -10), ("ke", -10), ("ko", -15), ("ky", -15), ("nv", -40), ("ov", -10), ("ow", -10),
        ("r,", -92), ("r-", -37), ("r.", -100), ("rc", -18), ("re", -18), ("rg", -10), ("rn", -15),
        ("ro", -18), ("rp", -10), ("rq", -18), ("rv", -10), ("v,", -55), ("v.", -70), ("va", -10),
        ("ve", -10), ("vo", -10), ("w,", -55), ("w.", -70), ("wo", -10), ("y,", -55), ("y.", -70),
        ("ye", -10), ("yo", -25),
    ],
    // italic
    &[
        ("AC", -30), ("AG", -35), ("AO", -40), ("AQ", -40), ("AT", -37), ("AU", -50), ("AV", -105),
        ("AW", -95), ("AY", -55), ("Au", -20), ("Av", -55), ("Aw", -55), ("Ay", -55), ("BA", -25),
        ("BU", -10), ("DA", -35), ("DV", -40), ("DW", -40), ("DY", -40), ("F,", -135), ("F.", -135),
        ("FA", -115), ("Fa", -75), ("Fe", -75), ("Fi", -45), ("Fo", -105), ("Fr", -55), ("J,", -25),
        ("J.", -25), ("JA", -40), ("Ja", -35), ("Je", -25), ("Jo", -25), ("Ju", -35), ("KO", -50),
        ("Ke", -35), ("Ko", -40), ("Ku", -40), ("Ky", -40), ("LT", -20), ("LV", -55), ("LW", -55),
        ("LY", -20), ("Ly", -30), ("NA", -27), ("OA", -55), ("OT", -40), ("OV", -50), ("OW", -50),
        ("OX", -40), ("OY", -50), ("P,", -135), ("P.", -135), ("PA", -90), ("Pa", -80), ("Pe", -80),
        ("Po", -80), ("QU", -10), ("RO", -40), ("RU", -40), ("RV", -18), ("RW", -18), ("RY", -18),
        ("T,", -74), ("T-", -74), ("T.", -74), ("T:", -55), ("T;", -65), ("TA", -50), ("TO", -18),
        ("Ta", -92), ("Te", -92), ("Ti", -55), ("To", -92), ("Tr", -55), ("Tu", -55), ("Tw", -74),
        ("Ty", -74), ("U,", -25), ("U.", -25), ("UA", -40), ("V,", -129), ("V-", -55), ("V.", -129),
        ("V:", -65), ("V;", -74), ("VA", -60), ("VO", -30), ("Va", -111), ("Ve", -111), ("Vi", -74),
        ("Vo", -111), ("Vu", -74), ("W,", -92), ("W-", -37), ("W.", -92), ("W:", -65), ("W;", -65),
        ("WA", -60), ("WO", -25), ("Wa", -92), ("We", -92), ("Wi", -55), ("Wo", -92), ("Wu", -55),
        ("Wy", -70), ("Y,", -92), ("Y-", -74), ("Y.", -92), ("Y:", -65), ("Y;", -65), ("YA", -50),
        ("YO", -15), ("Ya", -92), ("Ye", -92), ("Yi", -74), ("Yo", -92), ("Yu", -92), ("ag", -10),
        ("b.", -40), ("bu", -20), ("ch", -15), ("ck", -20), ("e,", -10), ("e.", -15), ("eg", -40),
        ("ev", -15), ("ew", -15), ("ex", -20), ("ey", -30), ("f,", -10), ("f.", -15), ("g,", -10),
        ("g.", -15), ("ge", -10), ("gg", -10), ("ke", -10), ("ko", -10), ("ky", -10), ("nv", -40),
        ("og", -10), ("ov", -10), ("r,", -111), ("r-", -20), ("r.", -111), ("ra", -15), ("rc", -37),
        ("rd", -37), ("re", -37), ("rg", -37), ("ro", -45), ("rq", -37), ("rs", -10), ("v,", -74),
        ("v.", -74), ("w,", -74), ("w.", -74), ("y,", -55), ("y.", -55),
    ],
    // bold italic
    &[
        ("AC", -65), ("AG", -60), ("AO", -50), ("AQ", -55), ("AT", -55), ("AU", -50), ("AV", -95),
        ("AW", -100), ("AY", -70), ("Au", -30), ("Av", -74), ("Aw", -74), ("Ay", -74), ("BA", -25),
        ("BU", -10), ("DA", -25), ("DV", -50), ("DW", -40), ("DY", -50), ("F,", -129), ("F.", -129),
        ("FA", -100), ("Fa", -95), ("Fe", -100), ("Fi", -40), ("Fo", -70), ("Fr", -50), ("J,", -10),
        ("J.", -10), ("JA", -25), ("Ja", -40), ("Je", -40), ("Jo", -40), ("Ju", -40), ("KO", -30),
        ("Ke", -25), ("Ko", -25), ("Ku", -20), ("Ky", -20), ("LT", -18), ("LV", -37), ("LW", -37),
        ("LY", -37), ("Ly", -37), ("NA", -30), ("OA", -40), ("OT", -40), ("OV", -50), ("OW", -50),
        ("OX", -40), ("OY", -50), ("P,", -129), ("P.", -129), ("PA", -85), ("Pa", -40), ("Pe", -50),
        ("Po", -55), ("QU", -10), ("RO", -40), ("RT", -30), ("RU", -40), ("RV", -18), ("RW", -18),
        ("RY", -18), ("T,", -92), ("T-", -92), ("T.", -92), ("T:", -74), ("T;", -74), ("TA", -55),
        ("TO", -18), ("Ta", -92), ("Te", -92), ("Ti", -37), ("To", -95), ("Tr", -37), ("Tu", -37),
        ("Tw", -37), ("Ty", -37), ("UA", -45), ("V,", -129), ("V-", -70), ("V.", -129), ("V:", -74),
        ("V;", -74), ("VA", -85), ("VG", -10), ("VO", -30), ("Va", -111), ("Ve", -111), ("Vi", -55),
        ("Vo", -111), ("Vu", -55), ("W,", -74), ("W-", -50), ("W.", -74), ("W:", -55), ("W;", -55),
        ("WA", -74), ("WO", -15), ("Wa", -85), ("We", -90), ("Wi", -37), ("Wo", -80), ("Wu", -55),
        ("Wy", -55), ("Y,", -92), ("Y-", -92), ("Y.", -74), ("Y:", -92), ("Y;", -92), ("YA", -74),
        ("YO", -25), ("Ya", -92), ("Ye", -111), ("Yi", -55), ("Yo", -111), ("Yu", -92), ("b.", -40),
        ("bb", -10), ("bu", -20), ("ch", -10), ("ck", -10), ("eb", -10), ("f,", -10), ("f.", -10),
        ("fe", -10), ("fo", -10), ("ke", -30), ("ko", -10), ("nv", -40), ("ov", -15), ("ow", -25),
        ("ox", -10), ("oy", -10), ("r,", -65), ("r.", -65), ("v,", -37), ("v.", -37), ("ve", -15),
        ("vo", -15), ("w,", -37), ("w.", -37), ("wa", -10), ("we", -10), ("wo", -15), ("xe", -10),
        ("y,", -37), ("y.", -37),
    ],
];

/// How much the pair `a` `b` in `style` is kerned, in font units.
fn kern(style: usize, a: char, b: char) -> f32 {
    let pair: String = [a, b].iter().collect();
    KERNS[style].binary_search_by(|(p, _)| (*p).cmp(pair.as_str())).map_or(0.0, |i| f32::from(KERNS[style][i].1))
}

/// The box with its letters in Termes (bold ones too where `bold`, as `\bfseries` sets the
/// punctuation): each moved by how much wider or narrower the ones before it are than in STIX Two,
/// and by TeX's kerning between neighbors.
pub fn retarget(mut b: MathBox, bold: bool) -> MathBox {
    let stix = TIMES.get();
    let map = characters();
    // left to right
    let mut order: Vec<usize> = (0..b.items.len()).collect();
    let x_of = |it: &Item| match it {
        Item::Glyph { x, .. } | Item::Rule { x, .. } => *x,
        Item::Line { x1, .. } => *x1,
        Item::Ellipse { cx, .. } => *cx,
        Item::Transformed { matrix, .. } => matrix[4],
        Item::Group { bounds, .. } => bounds[0],
    };
    order.sort_by(|&i, &j| x_of(&b.items[i]).total_cmp(&x_of(&b.items[j])));
    let mut shift = 0.0f32;
    // the last letter drawn in Termes on the line: its style, character, baseline and scale
    let mut last: Option<(usize, char, f32, f32)> = None;
    let (mut top, mut bottom, mut any) = (f32::MIN, f32::MIN, false);
    for i in order {
        match &mut b.items[i] {
            Item::Glyph { id, x, y, scale, .. } => {
                *x += shift;
                // (what Termes lacks, a math symbol, stays in STIX Two)
                let found = map.get(&id.0).and_then(|&(c, style)| {
                    let style = if style == Style::Regular && bold { Style::Bold } else { style } as usize;
                    face(style).glyph_index(c).map(|g| (c, style, g))
                });
                let Some((c, style, g)) = found else {
                    last = None;
                    continue;
                };
                let f = face(style);
                // (both fonts have 1000 units to the em, as mathra's boxes)
                if let Some((s, prev, py, ps)) = last {
                    if s == style && ps == *scale && py == *y {
                        let k = kern(style, prev, c) * *scale;
                        *x += k;
                        shift += k;
                    }
                }
                shift += (f32::from(f.glyph_hor_advance(g).unwrap_or(0)) - stix.advance(*id)) * *scale;
                if let Some(r) = f.glyph_bounding_box(g) {
                    top = top.max(*y + f32::from(r.y_max) * *scale);
                    bottom = bottom.max(-*y - f32::from(r.y_min) * *scale);
                }
                any = true;
                last = Some((style, c, *y, *scale));
                *id = GlyphId(FIRST | ((style as u16) << 12) | g.0);
            }
            Item::Rule { x, .. } => *x += shift,
            Item::Line { x1, x2, .. } => {
                *x1 += shift;
                *x2 += shift;
            }
            Item::Ellipse { cx, .. } => *cx += shift,
            // (a turned or scaled box keeps STIX Two's letters)
            Item::Transformed { matrix, .. } => matrix[4] += shift,
            // (a group with HTML attributes, `\class`, too: moved as one, through a translation)
            Item::Group { items, .. } => b.items[i] = Item::Transformed { matrix: [1.0, 0.0, 0.0, 1.0, shift, 0.0], items: std::mem::take(items) },
        }
    }
    b.width += shift;
    if any {
        // the height and depth of the letters' ink, as TeX's metrics of Times have them; what else
        // the box holds (a rule) as mathra measured it
        let others = b.items.iter().any(|it| !matches!(it, Item::Glyph { id, .. } if is_termes(*id)));
        b.height = if others { b.height.max(top) } else { top.max(0.0) };
        b.depth = if others { b.depth.max(bottom) } else { bottom.max(0.0) };
    }
    b
}

struct PathWriter(String);

fn num(v: f32) -> String {
    format!("{}", (v * 100.0).round() / 100.0)
}

impl OutlineBuilder for PathWriter {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "M{} {} ", num(x), num(y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "L{} {} ", num(x), num(y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.0, "Q{} {} {} {} ", num(x1), num(y1), num(x), num(y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.0, "C{} {} {} {} {} {} ", num(x1), num(y1), num(x2), num(y2), num(x), num(y));
    }
    fn close(&mut self) {
        self.0.push_str("Z ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadratic_curves_become_q() {
        // Termes is CFF (cubics only), but the outline writer takes any font
        let mut path = PathWriter(String::new());
        path.quad_to(1.0, 2.5, 3.0, 4.0);
        assert_eq!(path.0, "Q1 2.5 3 4 ");
    }
}
