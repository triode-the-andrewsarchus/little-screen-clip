// SPDX-License-Identifier: MPL-2.0
// Port of the D2 v0.9.0 lib/textmeasure Ruler pieces used by LayoutMarkdown:
// textmeasure.go (MeasurePrecise, drawBuf, controlRune, spaceWidth,
// measureFontWidth, measureFontAdvance, cssFontBoxMetrics,
// cssNormalLineHeight, scaleUnicodeCSS/Fallback, scaleUnicodeLegacy and the
// Unicode helpers), atlas.go / atlas_metrics.go (NewAtlas,
// makeMetricsSquareMapping, measureRune, DrawRune) and fontface.go
// (metricPreservingFace) over golang.org/x/image v0.45.0 sfnt/font
// fixed-point arithmetic (sfnt.scale, font.MeasureString, font.BoundString).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use d2_rs_text::d2fonts::{self, FontStyle};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FontFamily {
    SourceSansPro,
    SourceCodePro,
    HandDrawn,
    Registered(usize),
}

use d2_rs_text::measure::{TrueTypeFont, is_markdown_default_ignorable};
use d2_rs_text::uniseg;

/// d2fonts.Font: a face plus its integer size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FontSpec {
    pub family: FontFamily,
    pub style: FontStyle,
    pub size: i64,
}

impl FontSpec {
    pub(crate) fn new(family: FontFamily, size: i64, style: FontStyle) -> Self {
        Self {
            family,
            style,
            size,
        }
    }
}

const TAB_SIZE: f64 = 4.0;

/// The parsed bundled faces (NewRuler's ttfs map), shared process-wide.
fn ttf(family: FontFamily, style: FontStyle) -> Arc<TrueTypeFont> {
    static FACES: OnceLock<HashMap<(FontFamily, FontStyle), Arc<TrueTypeFont>>> = OnceLock::new();
    let faces = FACES.get_or_init(|| {
        let mut faces = HashMap::new();
        for (family, source) in [
            (
                FontFamily::SourceSansPro,
                d2fonts::FontFamily::SourceSansPro,
            ),
            (
                FontFamily::SourceCodePro,
                d2fonts::FontFamily::SourceCodePro,
            ),
            (FontFamily::HandDrawn, d2fonts::FontFamily::HandDrawn),
        ] {
            for style in d2fonts::FONT_STYLES {
                faces.insert(
                    (family, style),
                    Arc::new(
                        d2_rs_text::measure::bundled_face(source, style)
                            .expect("bundled D2 font parses"),
                    ),
                );
            }
        }
        faces
    });
    faces[&(family, style)].clone()
}

/// sfnt.scale: round half away from zero.
fn sfnt_scale(x: i64, units_per_em: i64) -> i64 {
    let x = if x >= 0 {
        x + units_per_em / 2
    } else {
        x - units_per_em / 2
    };
    x / units_per_em
}

/// fixed.Int26_6.Floor / Ceil.
fn floor26(x: i64) -> i64 {
    x >> 6
}
fn ceil26(x: i64) -> i64 {
    (x + 0x3f) >> 6
}
fn i2f(x: i64) -> f64 {
    x as f64 / 64.0
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Rect26 {
    min_x: i64,
    min_y: i64,
    max_x: i64,
    max_y: i64,
}

impl Rect26 {
    fn empty(&self) -> bool {
        self.min_x >= self.max_x || self.min_y >= self.max_y
    }
    /// fixed.Rectangle26_6.Union.
    fn union(self, s: Rect26) -> Rect26 {
        if self.empty() {
            return s;
        }
        if s.empty() {
            return self;
        }
        Rect26 {
            min_x: self.min_x.min(s.min_x),
            min_y: self.min_y.min(s.min_y),
            max_x: self.max_x.max(s.max_x),
            max_y: self.max_y.max(s.max_y),
        }
    }
}

/// metricPreservingFace at one size.
#[derive(Clone)]
struct Face {
    font: Arc<TrueTypeFont>,
    scale: i64,
}

impl Face {
    fn new(spec: FontSpec, font: Arc<TrueTypeFont>) -> Self {
        Self {
            font,
            scale: (0.5 + spec.size as f64 * 64.0) as i64,
        }
    }
    fn upem(&self) -> i64 {
        i64::from(self.font.units_per_em())
    }
    fn glyph_advance(&self, glyph: u16) -> i64 {
        sfnt_scale(
            i64::from(self.font.glyph_advance_units(glyph)) * self.scale,
            self.upem(),
        )
    }
    fn glyph_bounds(&self, glyph: u16) -> Rect26 {
        match self.font.glyph_outline_box(glyph) {
            None => Rect26::default(),
            Some((x0, y0, x1, y1)) => Rect26 {
                min_x: sfnt_scale(i64::from(x0) * self.scale, self.upem()),
                min_y: -sfnt_scale(i64::from(y1) * self.scale, self.upem()),
                max_x: sfnt_scale(i64::from(x1) * self.scale, self.upem()),
                max_y: -sfnt_scale(i64::from(y0) * self.scale, self.upem()),
            },
        }
    }
    /// metricPreservingFace.Kern: roundScale of the legacy kern pair.
    fn kern(&self, r0: char, r1: char) -> i64 {
        let Some(value) = self
            .font
            .kern_units(self.font.glyph_index(r0), self.font.glyph_index(r1))
        else {
            return 0;
        };
        let denominator = self.upem() * 64;
        let mut x = self.scale * i64::from(value) * 64;
        if x >= 0 {
            x += denominator / 2;
        } else {
            x -= denominator / 2;
        }
        x / denominator
    }
    fn ceil_scale(&self, units: i64) -> i64 {
        let denominator = self.upem() * 64;
        (self.scale * units + denominator - 1) / denominator
    }
    /// metricPreservingFace.Metrics: (Height, Ascent, Descent).
    fn metrics(&self) -> (i64, i64, i64) {
        let (ascent, descent, _) = self.font.hhea_metrics();
        (
            self.scale,
            self.ceil_scale(i64::from(ascent) * 64),
            self.ceil_scale(-i64::from(descent) * 64),
        )
    }
    fn supported(&self, s: &str) -> bool {
        s.chars()
            .all(|r| self.font.glyph_index(r) != 0 || r == '\0')
    }
}

#[derive(Clone, Copy)]
struct Glyph {
    dot: (f64, f64),
    frame: [f64; 4], // tl.x, tl.y, br.x, br.y (normalized)
    advance: f64,
}

struct Atlas {
    face: Face,
    mapping: HashMap<char, Glyph>,
    ascent: f64,
    descent: f64,
    line_height: f64,
}

fn atlas_runes() -> Vec<char> {
    // [ReplacementChar] followed by textmeasure.Runes (disjoint from it).
    std::iter::once('\u{fffd}')
        .chain(
            (0..=0xffu32)
                .chain(0x25a0..=0x25ffu32)
                .filter_map(char::from_u32),
        )
        .collect()
}

impl Atlas {
    /// NewAtlas(face, Runes) via makeMetricsSquareMapping.
    fn new(face: Face) -> Self {
        let glyphs: Vec<(char, Rect26, i64)> = atlas_runes()
            .into_iter()
            .map(|r| {
                let glyph = face.font.glyph_index(r);
                let b = face.glyph_bounds(glyph);
                let frame = Rect26 {
                    min_x: floor26(b.min_x) * 64,
                    min_y: floor26(b.min_y) * 64,
                    max_x: ceil26(b.max_x) * 64,
                    max_y: ceil26(b.max_y) * 64,
                };
                (r, frame, face.glyph_advance(glyph))
            })
            .collect();
        let (height, ascent, descent) = face.metrics();
        let line_height = ascent + descent;
        let padding = 2 * 64;
        let arrange =
            |width: i64, mapping: Option<&mut HashMap<char, ((i64, i64), Rect26, i64)>>| {
                let mut mapping = mapping;
                let mut bounds = Rect26::default();
                let (mut dot_x, mut dot_y) = (0i64, 0i64);
                for &(r, frame, advance) in &glyphs {
                    dot_x -= frame.min_x;
                    let frame = Rect26 {
                        min_x: frame.min_x + dot_x,
                        min_y: frame.min_y + dot_y,
                        max_x: frame.max_x + dot_x,
                        max_y: frame.max_y + dot_y,
                    };
                    if let Some(m) = mapping.as_deref_mut() {
                        m.insert(r, ((dot_x, dot_y), frame, advance));
                    }
                    bounds = bounds.union(frame);
                    dot_x = frame.max_x;
                    dot_x += padding;
                    dot_x = ceil26(dot_x) * 64;
                    if frame.max_x >= width {
                        dot_x = 0;
                        dot_y += line_height;
                        dot_y += padding;
                        dot_y = ceil26(dot_y) * 64;
                    }
                }
                bounds
            };
        // sort.Search over [0, fixed.I(1024*1024)).
        let (mut lo, mut hi) = (0i64, 1024 * 1024 * 64);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let b = arrange(mid, None);
            if b.max_x - b.min_x >= b.max_y - b.min_y {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let mut fixed_mapping = HashMap::new();
        let fixed_bounds = arrange(lo, Some(&mut fixed_mapping));
        let (tl_x, tl_y) = (i2f(fixed_bounds.min_x), i2f(fixed_bounds.min_y));
        let _ = tl_x;
        let br_y = i2f(fixed_bounds.max_y);
        let mapping = fixed_mapping
            .into_iter()
            .map(|(r, ((dx, dy), frame, advance))| {
                let (ax, ay) = (i2f(frame.min_x), br_y - (i2f(frame.min_y) - tl_y));
                let (bx, by) = (i2f(frame.max_x), br_y - (i2f(frame.max_y) - tl_y));
                (
                    r,
                    Glyph {
                        dot: (i2f(dx), br_y - (i2f(dy) - tl_y)),
                        frame: [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)],
                        advance: i2f(advance),
                    },
                )
            })
            .collect();
        Self {
            face,
            mapping,
            ascent: i2f(ascent),
            descent: i2f(descent),
            line_height: i2f(height),
        }
    }

    fn contains(&self, r: char) -> bool {
        self.mapping.contains_key(&r)
    }

    /// atlas.measureRune; prev None is Go's -1 (never contained).
    /// Returns (position, bounds) as [tl.x, tl.y, br.x, br.y].
    fn measure_rune(
        &self,
        prev: Option<char>,
        r: char,
        dot: &mut (f64, f64),
    ) -> ([f64; 4], [f64; 4]) {
        let r = if self.contains(r) { r } else { '\u{fffd}' };
        let prev = match prev {
            Some(p) if self.contains(p) => p,
            _ => '\u{fffd}',
        };
        dot.0 += i2f(self.face.kern(prev, r));
        let glyph = self.mapping[&r];
        let (x, y) = (dot.0 - glyph.dot.0, dot.1 - glyph.dot.1);
        let position = [
            glyph.frame[0] + x,
            glyph.frame[1] + y,
            glyph.frame[2] + x,
            glyph.frame[3] + y,
        ];
        let mut bounds = position;
        if (bounds[2] - bounds[0]) * (bounds[3] - bounds[1]) != 0.0 {
            bounds[1] = dot.1 - self.descent;
            bounds[3] = dot.1 + self.ascent;
        }
        dot.0 += glyph.advance;
        (position, bounds)
    }

    /// atlas.DrawRune's bounds result.
    fn draw_rune(&self, prev: Option<char>, r: char, dot: &mut (f64, f64)) -> [f64; 4] {
        let (position, measured) = self.measure_rune(prev, r, dot);
        if (position[2] - position[0]) * (position[3] - position[1]) != 0.0 {
            measured
        } else {
            position
        }
    }
}

/// textmeasure.Ruler restricted to the state LayoutMarkdown touches.
pub(crate) struct Ruler {
    pub line_height_factor: f64,
    pub bounds_with_dot: bool,
    atlases: HashMap<FontSpec, Atlas>,
    names: HashMap<String, FontFamily>,
    registered: HashMap<(FontFamily, FontStyle), Arc<TrueTypeFont>>,
}

fn union4(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

impl Ruler {
    pub(crate) fn new() -> Self {
        Self {
            line_height_factor: 1.0,
            bounds_with_dot: false,
            atlases: HashMap::new(),
            names: HashMap::new(),
            registered: HashMap::new(),
        }
    }

    pub(crate) fn family(&mut self, name: &str) -> Result<FontFamily, String> {
        match name {
            "SourceSansPro" => return Ok(FontFamily::SourceSansPro),
            "SourceCodePro" => return Ok(FontFamily::SourceCodePro),
            "HandDrawn" => return Ok(FontFamily::HandDrawn),
            _ => {}
        }
        if let Some(family) = self.names.get(name) {
            return Ok(*family);
        }
        let family = FontFamily::Registered(self.names.len());
        let mut faces = Vec::new();
        for style in d2fonts::FONT_STYLES {
            let data = d2fonts::registered_font_face(name, style)
                .ok_or_else(|| format!("font family {name} is not loaded"))?;
            let face = TrueTypeFont::parse(&data)
                .map_err(|e| format!("font {name}/{}: {e}", style.as_str()))?;
            faces.push((style, Arc::new(face)));
        }
        for (style, face) in faces {
            self.registered.insert((family, style), face);
        }
        self.names.insert(name.into(), family);
        Ok(family)
    }
    fn font(&self, spec: FontSpec) -> Arc<TrueTypeFont> {
        self.registered
            .get(&(spec.family, spec.style))
            .cloned()
            .unwrap_or_else(|| ttf(spec.family, spec.style))
    }
    fn atlas(&mut self, font: FontSpec) -> &Atlas {
        let face = self.font(font);
        self.atlases
            .entry(font)
            .or_insert_with(|| Atlas::new(Face::new(font, face)))
    }

    /// controlRune; true when r is a control rune.
    fn control_rune(&mut self, r: char, dot: &mut (f64, f64), font: FontSpec) -> bool {
        match r {
            '\n' => {
                let factor = self.line_height_factor;
                let line_height = self.atlas(font).line_height;
                dot.0 = 0.0;
                dot.1 -= factor * line_height;
            }
            '\r' => dot.0 = 0.0,
            '\t' => {
                let tab = self.atlas(font).mapping[&' '].advance * TAB_SIZE;
                let mut rem = dot.0 % tab;
                rem %= rem + tab;
                if rem == 0.0 {
                    rem = tab;
                }
                dot.0 += rem;
            }
            _ => return false,
        }
        true
    }

    /// Ruler.MeasurePrecise.
    pub(crate) fn measure_precise(&mut self, font: FontSpec, s: &str) -> (f64, f64) {
        self.atlas(font);
        let mut bounds = [0.0f64; 4];
        let mut dot = (0.0, 0.0);
        let mut prev = None;
        for r in s.chars() {
            if self.control_rune(r, &mut dot, font) {
                continue;
            }
            let (_, glyph_bounds) = self.atlases[&font].measure_rune(prev, r, &mut dot);
            prev = Some(r);
            if self.bounds_with_dot {
                bounds = union4(bounds, [dot.0, dot.1, dot.0, dot.1]);
            } else if (bounds[2] - bounds[0]) * (bounds[3] - bounds[1]) == 0.0 {
                bounds = glyph_bounds;
                continue;
            }
            bounds = union4(bounds, glyph_bounds);
        }
        (bounds[2] - bounds[0], bounds[3] - bounds[1])
    }

    /// Ruler.spaceWidth.
    pub(crate) fn space_width(&mut self, font: FontSpec) -> f64 {
        self.atlas(font).mapping[&' '].advance
    }

    /// Ruler.measureFontAdvance (font.MeasureString on the original face).
    pub(crate) fn measure_font_advance(&self, font: FontSpec, s: &str) -> Option<f64> {
        let face = Face::new(font, self.font(font));
        if !face.supported(s) {
            return None;
        }
        let mut advance = 0i64;
        let mut prev: Option<char> = None;
        for c in s.chars() {
            if let Some(p) = prev {
                advance += face.kern(p, c);
            }
            advance += face.glyph_advance(face.font.glyph_index(c));
            prev = Some(c);
        }
        Some(i2f(advance))
    }

    /// Ruler.measureFontWidth (font.BoundString on the original face).
    fn measure_font_width(&self, font: FontSpec, s: &str) -> Option<f64> {
        let face = Face::new(font, self.font(font));
        if !face.supported(s) {
            return None;
        }
        let mut bounds = Rect26::default();
        let mut advance = 0i64;
        let mut prev: Option<char> = None;
        for c in s.chars() {
            if let Some(p) = prev {
                advance += face.kern(p, c);
            }
            let glyph = face.font.glyph_index(c);
            let mut b = face.glyph_bounds(glyph);
            b.min_x += advance;
            b.max_x += advance;
            bounds = bounds.union(b);
            advance += face.glyph_advance(glyph);
            prev = Some(c);
        }
        let left = bounds.min_x.min(0);
        let right = bounds.max_x.max(advance);
        Some(i2f(right - left))
    }

    /// Ruler.cssFontBoxMetrics.
    pub(crate) fn css_font_box_metrics(&self, font: FontSpec, size: f64) -> Option<(f64, f64)> {
        let font = self.font(font);
        let ppem = (size * 64.0).round() as i64;
        let upem = i64::from(font.units_per_em());
        let (ascent, descent, _) = font.hhea_metrics();
        let a = sfnt_scale(i64::from(ascent) * ppem, upem);
        let d = -sfnt_scale(i64::from(descent) * ppem, upem);
        Some((i2f(a).round(), i2f(d).round()))
    }

    /// Ruler.cssNormalLineHeight.
    pub(crate) fn css_normal_line_height(&self, font: FontSpec, size: f64) -> Option<f64> {
        let font = self.font(font);
        let ppem = (size * 64.0).round() as i64;
        let upem = i64::from(font.units_per_em());
        let (ascent, descent, gap) = font.hhea_metrics();
        let h = sfnt_scale(
            (i64::from(ascent) - i64::from(descent) + i64::from(gap)) * ppem,
            upem,
        );
        Some(i2f(h).round())
    }

    /// atlas lineHeight and Ascent (textBaseline's fallback).
    pub(crate) fn atlas_line_height_ascent(&mut self, font: FontSpec) -> (f64, f64) {
        let atlas = self.atlas(font);
        (atlas.line_height, atlas.ascent)
    }

    /// Ruler.scaleUnicodeCSS.
    pub(crate) fn scale_unicode_css(&mut self, w: f64, font: FontSpec, s: &str) -> f64 {
        self.scale_unicode_fallback(w, font, s, true)
    }

    fn scale_unicode_fallback(
        &mut self,
        mut w: f64,
        font: FontSpec,
        s: &str,
        css_emoji: bool,
    ) -> f64 {
        if grapheme_cluster_count(s) == s.len() {
            return w;
        }
        w = 0.0;
        for line in s.split('\n') {
            let _ = self.measure_precise(font, line);
            if let Some(original) = self.measure_font_width(font, line) {
                w = w.max(original);
                continue;
            }
            let mut line_w = 0.0;
            for (grapheme, cells) in graphemes(line) {
                if let Some(width) = self.measure_font_width(font, grapheme) {
                    line_w += width;
                    continue;
                }
                let mut grapheme = grapheme.to_owned();
                let color_emoji = css_emoji && uniseg::is_color_emoji(&grapheme);
                if css_emoji && !color_emoji {
                    let filtered = strip_default_ignorable_runes(&grapheme);
                    if filtered != grapheme {
                        if filtered.is_empty() {
                            continue;
                        }
                        if let Some(width) = self.measure_font_width(font, &filtered) {
                            line_w += width;
                            continue;
                        }
                        grapheme = filtered;
                    }
                }
                if is_zero_width_grapheme(&grapheme) {
                    continue;
                }
                let cells = cells.max(1);
                let mut advance = font.size as f64 * cells as f64 / 2.0;
                if css_emoji {
                    if color_emoji {
                        advance = advance.max(font.size as f64 * 1.25);
                    } else if has_unicode_symbol(&grapheme) {
                        advance = advance.max(font.size as f64);
                    }
                }
                line_w += advance;
            }
            w = w.max(line_w);
        }
        w
    }

    /// Ruler.Measure: MeasurePrecise, scaleUnicode, then ceilings. (Added in
    /// the d2scenebuild copy for d2graph.GetTextDimensions.)
    pub(crate) fn measure(&mut self, font: FontSpec, s: &str) -> (i64, i64) {
        let (w, h) = self.measure_precise(font, s);
        let w = self.scale_unicode(w, font, s);
        (w.ceil() as i64, h.ceil() as i64)
    }

    /// Ruler.scaleUnicode (textmeasure.go): D2's established approximation
    /// for ordinary labels.
    pub(crate) fn scale_unicode(&mut self, mut w: f64, font: FontSpec, s: &str) -> f64 {
        if grapheme_cluster_count(s) == s.len() {
            return w;
        }
        let mono = FontSpec::new(FontFamily::SourceCodePro, font.size, font.style);
        for line in s.split('\n') {
            let (mut line_w, _) = self.measure_precise(font, line);
            for (grapheme, cells) in graphemes(line) {
                if cells == 1 {
                    continue;
                }
                let mut previous = Some('\0');
                let mut dot = (0.0, 0.0);
                let mut bounds = [0.0f64; 4];
                for r in grapheme.chars() {
                    if self.control_rune(r, &mut dot, font) {
                        continue;
                    }
                    let glyph_bounds = self.atlases[&font].draw_rune(previous, r, &mut dot);
                    bounds = union4(bounds, glyph_bounds);
                    previous = Some(r);
                }
                line_w -= bounds[2] - bounds[0];
                line_w += self.space_width(mono) * cells as f64;
            }
            w = w.max(line_w);
        }
        w
    }

    /// Ruler.scaleUnicodeLegacy.
    pub(crate) fn scale_unicode_legacy(&mut self, mut w: f64, font: FontSpec, s: &str) -> f64 {
        if grapheme_cluster_count(s) == s.len() {
            return w;
        }
        let (measurement_text, filtered) = legacy_measurement_text(s);
        if filtered {
            w = 0.0;
        }
        let mono = FontSpec::new(FontFamily::SourceCodePro, font.size, font.style);
        for line in measurement_text.split('\n') {
            let (mut line_w, _) = self.measure_precise(font, line);
            let line_floor = line_w;
            for (grapheme, cells) in graphemes(line) {
                if cells == 1 {
                    continue;
                }
                let mut previous = Some('\0');
                let mut dot = (0.0, 0.0);
                let mut bounds = [0.0f64; 4];
                for r in grapheme.chars() {
                    if self.control_rune(r, &mut dot, font) {
                        continue;
                    }
                    let glyph_bounds = self.atlases[&font].draw_rune(previous, r, &mut dot);
                    bounds = union4(bounds, glyph_bounds);
                    previous = Some(r);
                }
                line_w -= bounds[2] - bounds[0];
                line_w += self.space_width(mono) * cells as f64;
            }
            if filtered {
                line_w = line_w.max(line_floor);
            }
            w = w.max(line_w);
        }
        w
    }
}

/// uniseg graphemes with their Width().
pub(crate) fn graphemes(s: &str) -> Vec<(&str, usize)> {
    let mut start = 0;
    uniseg::grapheme_boundaries(s)
        .into_iter()
        .map(|end| {
            let g = &s[start..end];
            start = end;
            (g, uniseg::string_width(g))
        })
        .collect()
}

pub(crate) fn grapheme_cluster_count(s: &str) -> usize {
    uniseg::grapheme_boundaries(s).len()
}

pub(crate) fn strip_default_ignorable_runes(s: &str) -> String {
    s.chars()
        .filter(|&r| !is_markdown_default_ignorable(r))
        .collect()
}

pub(crate) fn has_unicode_symbol(s: &str) -> bool {
    s.chars().any(uniseg::is_symbol)
}

pub(crate) fn is_zero_width_grapheme(s: &str) -> bool {
    s.chars()
        .all(|r| uniseg::is_mark(r) || is_markdown_default_ignorable(r))
}

fn is_default_ignorable_grapheme(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_markdown_default_ignorable)
}

fn legacy_measurement_text(s: &str) -> (String, bool) {
    let mut out = String::new();
    let mut filtered = false;
    for (cluster, width) in graphemes(s) {
        if is_default_ignorable_grapheme(cluster) {
            filtered = true;
            continue;
        }
        if width != 1 {
            out.push_str(cluster);
            continue;
        }
        for r in cluster.chars() {
            if is_markdown_default_ignorable(r) {
                filtered = true;
                continue;
            }
            out.push(r);
        }
    }
    if !filtered {
        return (s.to_owned(), false);
    }
    (out, true)
}
