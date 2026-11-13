// SPDX-License-Identifier: MPL-2.0
//! markdown.go: Markdown labels as clipped groups of typed primitives over
//! lib/textmeasure LayoutMarkdown (golib::markdown, copied from d2-rs-render).
//! ARM64 contraction: markdown.go:294 (`Y + FontSize*.25`) and the five
//! inlined Matrix.Mul of the textLength/synthetic-italic transforms.

use d2_rs_scene::{
    Bounds, Box as SceneBox, Font, LineCap, LineJoin, Matrix, Node, Paint, Path, PathCommand,
    Point, Primitive, Rect, Stroke, TextAnchor, TextRun,
};
use d2_rs_target::{Connection, Shape};

use crate::Builder;
use crate::golib::geo::{self, go_fmadd};
use crate::golib::label::{self, Position};
use crate::golib::markdown::paint::{
    ColorRole, FontRole, MarkdownLayout, Primitive as MdPrimitive, PrimitiveKind, layout_markdown,
};
use crate::golib::markdown::ruler::Ruler;
use crate::labels::expanded_label_box;
use crate::target_ext::*;
use crate::util::*;

const MAX_MARKDOWN_SOURCE_BYTES: usize = 1 << 20;
pub(crate) const MAX_MARKDOWN_PRIMITIVES: usize = 100_000;
/// Go's constant `-12 * math.Pi / 180`, rounded once from the exact value.
const SYNTHETIC_ITALIC_SKEW: f64 = -0.209_439_510_239_319_56;

/// validateMarkdownLabel.
pub(crate) fn validate_markdown_label(
    object: &str,
    source: &str,
    font_size: i64,
    width: i64,
    height: i64,
) -> Result<()> {
    // Rust strings are valid UTF-8 (the JSON decoder already replaced
    // invalid sequences, as encoding/json does).
    if source.len() > MAX_MARKDOWN_SOURCE_BYTES {
        return Err(invalid_field(
            object,
            "label",
            Some(&source.len().to_string()),
            &format!("Markdown source bytes must not exceed {MAX_MARKDOWN_SOURCE_BYTES}"),
        ));
    }
    if width > 0 && height > 0 && font_size <= 0 {
        return Err(invalid_field(
            object,
            "fontSize",
            Some(&font_size.to_string()),
            "must be positive for a Markdown label",
        ));
    }
    Ok(())
}

/// The primary/mono families for a label font name (markdownFontFamilies).
pub(crate) fn markdown_font_families(
    diagram_font: Option<&str>,
    diagram_mono: Option<&str>,
    font_name: &str,
) -> (String, String) {
    let mut primary = diagram_font.unwrap_or("SourceSansPro").to_owned();
    let mono = diagram_mono.unwrap_or("SourceCodePro").to_owned();
    if font_name.eq_ignore_ascii_case("mono") {
        primary = mono.clone();
    } else if !font_name.is_empty() && !font_name.eq_ignore_ascii_case("default") {
        match font_name.to_lowercase().as_str() {
            "default" => primary = "SourceSansPro".into(),
            "mono" => primary = "SourceCodePro".into(),
            _ => {}
        }
    }
    (primary, mono)
}

/// LayoutMarkdown over named families.
pub(crate) fn layout(
    ruler: &mut Ruler,
    source: &str,
    primary: &str,
    mono: &str,
    font_size: i64,
) -> std::result::Result<MarkdownLayout, String> {
    let primary = ruler.family(primary)?;
    let mono = ruler.family(mono)?;
    layout_markdown(source, ruler, primary, mono, font_size)
}

/// textmeasure.MeasureMarkdown with a fresh ruler (calculateTooltipBounds).
pub(crate) fn measure_markdown(
    source: &str,
    diagram_font: Option<&str>,
    diagram_mono: Option<&str>,
    font_name: &str,
    font_size: i64,
) -> std::result::Result<(i64, i64), String> {
    let (primary, mono) = markdown_font_families(diagram_font, diagram_mono, font_name);
    let mut ruler = Ruler::new();
    layout(&mut ruler, source, &primary, &mono, font_size).map(|l| (l.width, l.height))
}

/// markdownShapeLabelTopLeft.
fn markdown_shape_label_top_left(s: &Shape) -> geo::Point {
    let mut position = Position::from_string(&s.label_position);
    if position == Position::Unset {
        position = Position::InsideMiddleCenter;
    }
    let b = expanded_label_box(s, position);
    position.get_point_on_box(
        &b,
        label::PADDING,
        s.text.label_width as f64,
        s.text.label_height as f64,
    )
}

/// textmeasure.SafeMarkdownLink (master: IsDangerousLink after normalization).
pub(crate) fn safe_markdown_link(link: &str) -> &str {
    let link = link.trim();
    if link.is_empty() || is_dangerous_link(link) {
        return "";
    }
    link
}

/// textmeasure.IsDangerousLink.
pub fn is_dangerous_link(link: &str) -> bool {
    let (normalized, complete) = normalize_link_for_safety(link);
    if !complete {
        return true;
    }
    is_dangerous_url(normalized.as_bytes())
}

fn normalize_link_for_safety(link: &str) -> (String, bool) {
    let mut normalized = link.trim().to_owned();
    for _ in 0..8 {
        let unescaped = String::from_utf8_lossy(&d2_rs_text::markdown::html::unescape_html(
            normalized.as_bytes(),
        ))
        .into_owned();
        let decoded = decode_link_percent_escapes(&unescaped);
        let next: String = decoded
            .chars()
            .filter(|r| !r.is_control() && !r.is_whitespace())
            .collect();
        if has_url_scheme(&next) || next == normalized {
            return (next, true);
        }
        normalized = next;
    }
    (normalized, false)
}

fn has_url_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(colon) = value.find(':') else {
        return false;
    };
    if colon == 0 || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    bytes[1..colon]
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
}

fn decode_link_percent_escapes(value: &str) -> String {
    let bytes = value.as_bytes();
    if !bytes.contains(&b'%') {
        return value.to_owned();
    }
    let hex = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 2 < bytes.len() && bytes[i] == b'%' {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Go strings hold arbitrary bytes; later classification only inspects
    // ASCII prefixes, so a lossy view is equivalent here.
    String::from_utf8_lossy(&out).into_owned()
}

/// goldmark renderer/html.IsDangerousURL.
fn is_dangerous_url(url: &[u8]) -> bool {
    let has_prefix = |s: &[u8], prefix: &[u8]| {
        s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
    };
    if has_prefix(url, b"data:image/") && url.len() >= 11 {
        let v = &url[11..];
        return !(has_prefix(v, b"png;")
            || has_prefix(v, b"gif;")
            || has_prefix(v, b"jpeg;")
            || has_prefix(v, b"webp;"));
    }
    has_prefix(url, b"javascript:")
        || has_prefix(url, b"vbscript:")
        || has_prefix(url, b"file:")
        || has_prefix(url, b"data:")
}

struct RolePaints {
    foreground: Option<Paint>,
    muted: Option<Paint>,
    accent: Option<Paint>,
    border: Option<Paint>,
    border_muted: Option<Paint>,
    canvas: Option<Paint>,
    canvas_subtle: Option<Paint>,
    neutral_muted: Option<Paint>,
}

impl RolePaints {
    fn get(&self, role: ColorRole) -> Option<Paint> {
        match role {
            ColorRole::None => None,
            ColorRole::Foreground | ColorRole::ForegroundStroke => self.foreground.clone(),
            ColorRole::Muted | ColorRole::MutedStroke => self.muted.clone(),
            ColorRole::Accent => self.accent.clone(),
            ColorRole::Border => self.border.clone(),
            ColorRole::BorderMuted => self.border_muted.clone(),
            ColorRole::Canvas => self.canvas.clone(),
            ColorRole::CanvasSubtle => self.canvas_subtle.clone(),
            ColorRole::NeutralMuted => self.neutral_muted.clone(),
        }
    }
}

fn markdown_estimated_advance(text: &str, font_size: f64) -> f64 {
    let count = text.chars().count();
    if count == 0 {
        return geo::go_max(1.0, font_size * 0.25);
    }
    geo::go_max(1.0, count as f64 * font_size * 0.6)
}

fn intersect_markdown_bounds(left: Bounds, right: Bounds) -> Bounds {
    if !left.valid || !right.valid {
        return Bounds::default();
    }
    let min_x = geo::go_max(left.min.x, right.min.x);
    let min_y = geo::go_max(left.min.y, right.min.y);
    let max_x = geo::go_min(left.max.x, right.max.x);
    let max_y = geo::go_min(left.max.y, right.max.y);
    if max_x <= min_x || max_y <= min_y {
        return Bounds::default();
    }
    Bounds::new(min_x, min_y, max_x, max_y)
}

impl Builder<'_> {
    pub(crate) fn markdown_ruler(&mut self) -> &mut Ruler {
        self.markdown_ruler.get_or_insert_with(Ruler::new)
    }

    pub(crate) fn markdown_font_families_for(&self, font_name: &str) -> (String, String) {
        markdown_font_families(
            self.diagram.font_family.as_deref(),
            self.diagram.mono_font_family.as_deref(),
            font_name,
        )
    }

    pub(crate) fn build_shape_markdown(&mut self, s: &Shape) -> Result<Vec<Node>> {
        let top_left = markdown_shape_label_top_left(s);
        let node = self.build_markdown_label(
            &format!("shape {}", q(&s.id)),
            &format!("{}:markdown", s.id),
            &s.text.label,
            &s.text.font_family,
            s.text.font_size,
            s.text.label_width,
            s.text.label_height,
            &s.font_color(),
            &s.fill,
            !s.link.is_empty(),
            s.text.underline,
            top_left,
        )?;
        Ok(vec![node])
    }

    pub(crate) fn build_connection_markdown(
        &mut self,
        c: &Connection,
        top_left: geo::Point,
    ) -> Result<Node> {
        self.build_markdown_label(
            &format!("connection {}", q(&c.id)),
            &format!("{}:markdown", c.id),
            &c.text.label,
            &c.text.font_family,
            c.text.font_size,
            c.text.label_width,
            c.text.label_height,
            &connection_font_color(c),
            &c.fill,
            !c.link.is_empty(),
            c.text.underline,
            top_left,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_markdown_label(
        &mut self,
        object: &str,
        id: &str,
        source: &str,
        font_name: &str,
        font_size: i64,
        width: i64,
        height: i64,
        foreground: &str,
        background: &str,
        disable_links: bool,
        underline: bool,
        top_left: geo::Point,
    ) -> Result<Node> {
        let mut root = node(id, None);
        root.transform = Matrix::translate(top_left.x, top_left.y);
        let (primary, mono) = self.markdown_font_families_for(font_name);
        let layout_font_size = if font_size <= 0 { 16 } else { font_size };
        let markdown_layout = layout(
            self.markdown_ruler(),
            source,
            &primary,
            &mono,
            layout_font_size,
        )
        .map_err(|e| Error(format!("scene: {object} Markdown layout: {e}")))?;
        if markdown_layout.primitives.len() > MAX_MARKDOWN_PRIMITIVES {
            return err(format!(
                "scene: {object} Markdown primitive count {} exceeds limit {MAX_MARKDOWN_PRIMITIVES}",
                markdown_layout.primitives.len()
            ));
        }
        if width == 0 || height == 0 {
            return Ok(root);
        }
        let paints = self
            .markdown_role_paints(foreground)
            .map_err(|e| Error(format!("scene: {object} Markdown paints: {e}")))?;
        let viewport = SceneBox {
            x: 0.0,
            y: 0.0,
            width: width as f64,
            height: height as f64,
        };
        root.clip = Some(box_clip(viewport));
        let mut children = Vec::new();
        if !background.is_empty()
            && !background.eq_ignore_ascii_case("none")
            && !background.eq_ignore_ascii_case("transparent")
        {
            if let Some(fill) = self.paint(background, &format!("{object} Markdown background"))? {
                children.push(node(
                    format!("{id}:background"),
                    Some(Primitive::Rect(Rect {
                        r#box: viewport,
                        fill: Some(fill),
                        ..Default::default()
                    })),
                ));
            }
        }
        for (index, primitive) in markdown_layout.primitives.iter().enumerate() {
            let (n, link_box) = self
                .build_markdown_primitive(id, index, primitive, &primary, &mono, &paints, underline)
                .map_err(|e| Error(format!("scene: {object} Markdown primitive {index}: {e}")))?;
            if let Some(n) = n {
                children.push(n);
            }
            let link = safe_markdown_link(&primitive.link);
            if disable_links || link.is_empty() || !link_box.valid {
                continue;
            }
            let link_box = intersect_markdown_bounds(link_box, viewport.bounds());
            if !link_box.valid {
                continue;
            }
            self.add_markdown_link_region(
                object,
                index,
                link,
                &primitive.link_title,
                link_box
                    .translate(Point::new(top_left.x, top_left.y))
                    .to_box(),
            )?;
        }
        root.children = opt_vec(children);
        Ok(root)
    }

    fn markdown_role_paints(&self, foreground: &str) -> std::result::Result<RolePaints, String> {
        let paint = |value: &str, role: &str| {
            self.paint(value, &format!("Markdown {role}"))
                .map_err(|e| e.0)
        };
        let foreground_paint = paint(foreground, "foreground")?;
        paint(foreground, "foreground-stroke")?;
        let muted = paint("N2", "muted")?;
        paint("N2", "muted-stroke")?;
        Ok(RolePaints {
            foreground: foreground_paint,
            muted,
            accent: paint("B2", "accent")?,
            border: paint("B1", "border")?,
            border_muted: paint("B2", "border-muted")?,
            canvas: paint("N7", "canvas")?,
            canvas_subtle: paint("N6", "canvas-subtle")?,
            neutral_muted: paint("N6", "neutral-muted")?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_markdown_primitive(
        &mut self,
        id: &str,
        index: usize,
        primitive: &MdPrimitive,
        primary: &str,
        mono: &str,
        paints: &RolePaints,
        underline: bool,
    ) -> std::result::Result<(Option<Node>, Bounds), String> {
        let fill = paints.get(primitive.fill_role);
        let stroke_paint = paints.get(primitive.stroke_role);
        let node_id = format!("{id}:primitive:{index}");
        match primitive.kind {
            PrimitiveKind::Rect => {
                let b = SceneBox {
                    x: primitive.x,
                    y: primitive.y,
                    width: primitive.width,
                    height: primitive.height,
                };
                let stroke = stroke_paint
                    .filter(|_| primitive.stroke_width > 0.0)
                    .map(|paint| Stroke {
                        paint: Some(paint),
                        width: primitive.stroke_width,
                        cap: LineCap::BUTT,
                        join: LineJoin::MITER,
                        miter_limit: 4.0,
                        ..Default::default()
                    });
                let n = node(
                    node_id,
                    Some(Primitive::Rect(Rect {
                        r#box: b,
                        radius_x: primitive.radius,
                        radius_y: primitive.radius,
                        fill,
                        stroke,
                    })),
                );
                Ok((Some(n), b.bounds()))
            }
            PrimitiveKind::Line => {
                let Some(stroke_paint) = stroke_paint.filter(|_| primitive.stroke_width > 0.0)
                else {
                    return Ok((None, Bounds::default()));
                };
                let stroke = Stroke {
                    paint: Some(stroke_paint),
                    width: primitive.stroke_width,
                    cap: LineCap::BUTT,
                    join: LineJoin::MITER,
                    miter_limit: 4.0,
                    ..Default::default()
                };
                let path = Path {
                    commands: vec![
                        PathCommand::move_to(primitive.x, primitive.y),
                        PathCommand::line_to(primitive.x2, primitive.y2),
                    ],
                    stroke: Some(stroke),
                    ..Default::default()
                };
                let bounds = Bounds::from_points(&[
                    Point::new(primitive.x, primitive.y),
                    Point::new(primitive.x2, primitive.y2),
                ])
                .expand(primitive.stroke_width / 2.0, primitive.stroke_width / 2.0);
                Ok((Some(node(node_id, Some(Primitive::Path(path)))), bounds))
            }
            PrimitiveKind::Text => {
                let font_value =
                    self.markdown_font(primitive.font, primary, mono, primitive.font_size)?;
                let width = if primitive.width <= 0.0 {
                    markdown_estimated_advance(&primitive.text, primitive.font_size)
                } else {
                    primitive.width
                };
                let height = if primitive.height <= 0.0 {
                    primitive.font_size
                } else {
                    primitive.height
                };
                // ARM64 markdown.go:294: FMADDD.
                let ink = Bounds::new(
                    primitive.x,
                    primitive.y - height,
                    primitive.x + width,
                    go_fmadd(primitive.font_size, 0.25, primitive.y),
                );
                let text_stroke = fill
                    .clone()
                    .filter(|_| primitive.synthetic_bold)
                    .map(|paint| Stroke {
                        paint: Some(paint),
                        width: geo::go_max(0.35, primitive.font_size / 32.0),
                        cap: LineCap::ROUND,
                        join: LineJoin::ROUND,
                        miter_limit: 4.0,
                        ..Default::default()
                    });
                let run = TextRun {
                    text: primitive.text.clone(),
                    origin: Point::new(primitive.x, primitive.y),
                    anchor: TextAnchor::START,
                    font: font_value.clone(),
                    fill,
                    stroke: text_stroke,
                    underline,
                    strike: primitive.line_through,
                    ink,
                    ..Default::default()
                };
                let mut n = node(node_id, Some(Primitive::TextRun(run)));
                let mut transform = Matrix::identity();
                if primitive.text_length && primitive.width > 0.0 {
                    let advance = crate::shaping::markdown_font_advance(
                        self,
                        &font_value,
                        &primitive.text,
                        primitive.font_size,
                    )
                    .map_err(|e| format!("measure textLength: {e}"))?;
                    if advance <= 0.0 {
                        return Err("textLength source has zero advance".into());
                    }
                    let scale_x = primitive.width / advance;
                    if !finite(scale_x) || scale_x <= 0.0 {
                        return Err(format!(
                            "textLength produces invalid horizontal scale {}",
                            fv(scale_x)
                        ));
                    }
                    transform = Matrix::translate(primitive.x, primitive.y)
                        .mul(Matrix::scale(scale_x, 1.0))
                        .mul(Matrix::translate(-primitive.x, -primitive.y));
                }
                if primitive.synthetic_italic {
                    let italic = Matrix::translate(primitive.x, primitive.y)
                        .mul(Matrix::skew_x(SYNTHETIC_ITALIC_SKEW))
                        .mul(Matrix::translate(-primitive.x, -primitive.y));
                    transform = italic.mul(transform);
                }
                n.transform = transform;
                Ok((Some(n), ink.transform(transform)))
            }
        }
    }

    fn markdown_font(
        &mut self,
        role: FontRole,
        primary: &str,
        mono: &str,
        size: f64,
    ) -> std::result::Result<Font, String> {
        let (family, style) = match role {
            FontRole::Regular => (primary, "regular"),
            FontRole::Semibold => (primary, "semibold"),
            FontRole::Bold => (primary, "bold"),
            FontRole::Italic => (primary, "italic"),
            FontRole::Mono => (mono, "regular"),
            FontRole::MonoSemibold => (mono, "semibold"),
            FontRole::MonoBold => (mono, "bold"),
            FontRole::MonoItalic => (mono, "italic"),
        };
        self.font_for(family, style, size)
    }
}
