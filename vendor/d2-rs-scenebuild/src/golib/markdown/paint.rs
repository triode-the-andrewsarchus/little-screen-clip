// SPDX-License-Identifier: MPL-2.0
// Port of D2 v0.9.0 lib/textmeasure/markdown_svg.go: LayoutMarkdown, the
// markdownPainter (paintBlock, paintFlow, flowUnits, paintInlineNodes,
// wrapInlineTokens, atomizeInlineTokens, collectInlineTokens, paintPre,
// paintTable, list markers, textBaseline) and MarkdownLayout.SVG.

use std::collections::BTreeMap;

use super::ruler::FontFamily;
use d2_rs_text::d2fonts::FontStyle;
use d2_rs_text::uniseg;

use super::dom::{self, Dom, NodeId, NodeType};
use super::measure::*;
use super::ruler::{self, FontSpec, Ruler};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ColorRole {
    None,
    Foreground,
    ForegroundStroke,
    Muted,
    MutedStroke,
    Accent,
    Border,
    BorderMuted,
    Canvas,
    CanvasSubtle,
    NeutralMuted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FontRole {
    Regular,
    Semibold,
    Bold,
    Italic,
    Mono,
    MonoSemibold,
    MonoBold,
    MonoItalic,
}

impl FontRole {
    /// defaultMarkdownSVGFontClasses.
    pub(crate) fn class(self) -> &'static str {
        match self {
            FontRole::Regular => "text",
            FontRole::Semibold => "text-semibold",
            FontRole::Bold => "text-bold",
            FontRole::Italic => "text-italic",
            FontRole::Mono => "text-mono",
            FontRole::MonoSemibold => "text-mono-semibold",
            FontRole::MonoBold => "text-mono-bold",
            FontRole::MonoItalic => "text-mono-italic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrimitiveKind {
    Text,
    Rect,
    Line,
}

/// MarkdownPrimitive.
#[derive(Clone, Debug)]
pub(crate) struct Primitive {
    pub kind: PrimitiveKind,
    pub x: f64,
    pub y: f64,
    pub x2: f64,
    pub y2: f64,
    pub width: f64,
    pub height: f64,
    pub radius: f64,
    pub stroke_width: f64,
    pub text: String,
    pub font: FontRole,
    pub font_size: f64,
    pub fill_role: ColorRole,
    pub stroke_role: ColorRole,
    pub link: String,
    pub link_title: String,
    pub line_through: bool,
    pub synthetic_bold: bool,
    pub synthetic_italic: bool,
    pub text_length: bool,
}

impl Primitive {
    fn rect(x: f64, y: f64, width: f64, height: f64, fill_role: ColorRole) -> Self {
        Self {
            kind: PrimitiveKind::Rect,
            x,
            y,
            x2: 0.0,
            y2: 0.0,
            width,
            height,
            radius: 0.0,
            stroke_width: 0.0,
            text: String::new(),
            font: FontRole::Regular,
            font_size: 0.0,
            fill_role,
            stroke_role: ColorRole::None,
            link: String::new(),
            link_title: String::new(),
            line_through: false,
            synthetic_bold: false,
            synthetic_italic: false,
            text_length: false,
        }
    }
    fn text(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        text: String,
        ctx: &Ctx,
        font_size: f64,
    ) -> Self {
        Self {
            kind: PrimitiveKind::Text,
            text,
            font: ctx.font_role(),
            font_size,
            fill_role: ctx.role,
            synthetic_bold: ctx.synthetic_bold(),
            synthetic_italic: ctx.synthetic_italic(),
            ..Self::rect(x, y, width, height, ColorRole::None)
        }
    }
}

/// MarkdownLayout.
#[derive(Clone, Debug, Default)]
pub(crate) struct MarkdownLayout {
    pub width: i64,
    pub height: i64,
    pub primitives: Vec<Primitive>,
    pub corpus: String,
}

/// markdownPaintContext.
#[derive(Clone, Debug)]
struct Ctx {
    font_family: FontFamily,
    mono_font_family: FontFamily,
    font_size: i64,
    font_scale: f64,
    font_style: FontStyle,
    bold: bool,
    italic: bool,
    line_height_factor: f64,
    mono: bool,
    inline_code: bool,
    heading_code: bool,
    heading: String,
    role: ColorRole,
    link: String,
    link_title: String,
    line_through: bool,
    decoration_group: i32,
    decoration_role: ColorRole,
    table_cell: bool,
    text_align: String,
}

impl Ctx {
    fn css_font_size(&self) -> f64 {
        self.font_size as f64 * self.font_scale
    }
    fn synthetic_bold(&self) -> bool {
        self.bold && self.font_style != FontStyle::Bold && self.font_style != FontStyle::Semibold
    }
    fn synthetic_italic(&self) -> bool {
        self.italic && self.font_style != FontStyle::Italic
    }
    fn font_role(&self) -> FontRole {
        match (self.mono, self.font_style) {
            (true, FontStyle::Semibold) => FontRole::MonoSemibold,
            (true, FontStyle::Bold) => FontRole::MonoBold,
            (true, FontStyle::Italic) => FontRole::MonoItalic,
            (true, FontStyle::Regular) => FontRole::Mono,
            (false, FontStyle::Semibold) => FontRole::Semibold,
            (false, FontStyle::Bold) => FontRole::Bold,
            (false, FontStyle::Italic) => FontRole::Italic,
            (false, FontStyle::Regular) => FontRole::Regular,
        }
    }
    /// The face `fontFamily.Font(ctx.fontSize, ctx.effectiveFontStyle())`
    /// selected with the mono override.
    fn paint_font(&self) -> FontSpec {
        let family = if self.mono {
            self.mono_font_family
        } else {
            self.font_family
        };
        FontSpec::new(family, self.font_size, self.font_style)
    }
}

#[derive(Clone, Debug, Default)]
struct Token {
    break_line: bool,
    break_before: bool,
    break_after_soft_hyphen: bool,
    soft_hyphen_end: bool,
    discretionary_hyphen: bool,
    padding: bool,
    leading_padding: bool,
    text: String,
    width: f64,
    height: f64,
    ctx: Option<Ctx>,
    code_group: i32,
    code_height: f64,
    decoration_group: i32,
}

impl Token {
    fn ctx(&self) -> &Ctx {
        self.ctx.as_ref().expect("token context")
    }
}

struct FlowUnit {
    block: Option<NodeId>,
    inline: Vec<NodeId>,
    attrs: BlockAttrs,
}

struct Painter<'a> {
    ruler: &'a mut Ruler,
    dom: &'a Dom,
    primitives: Vec<Primitive>,
}

/// markdownSnap.
fn snap(v: f64) -> f64 {
    (v + 0.5).floor()
}

impl Painter<'_> {
    fn measure(&mut self, n: NodeId, ctx: &Ctx) -> BlockAttrs {
        let original = self.ruler.line_height_factor;
        self.ruler.line_height_factor = ctx.line_height_factor;
        let attrs = measure_markdown_node(
            self.ruler,
            self.dom,
            n,
            MeasureArgs {
                font_family: ctx.font_family,
                mono_font_family: ctx.mono_font_family,
                font_size: ctx.font_size,
                font_scale: ctx.font_scale,
                font_style: ctx.font_style,
                bold: ctx.bold,
                italic: ctx.italic,
                css: true,
            },
        );
        self.ruler.line_height_factor = original;
        attrs
    }

    fn child_context(&self, n: NodeId, ctx: &Ctx) -> Ctx {
        let mut ctx = ctx.clone();
        let node = self.dom.node(n);
        if node.kind != NodeType::Element {
            return ctx;
        }
        match node.data.as_str() {
            h @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                ctx.heading = h.to_owned();
                ctx.font_scale *= header_to_font_scale(h);
                ctx.font_style = FontStyle::Semibold;
                ctx.bold = false;
                ctx.line_height_factor = LINE_HEIGHT_H;
                if h == "h6" {
                    ctx.role = ColorRole::Muted;
                }
            }
            "em" => {
                ctx.font_style = FontStyle::Italic;
                ctx.italic = true;
            }
            "b" | "strong" => {
                ctx.font_style = FontStyle::Bold;
                ctx.bold = true;
            }
            "pre" | "code" => {
                ctx.font_family = ctx.mono_font_family;
                ctx.mono = true;
                ctx.font_style = FontStyle::Regular;
                ctx.inline_code = node.data == "code"
                    && node.parent.is_none_or(|p| self.dom.node(p).data != "pre");
                ctx.heading_code = ctx.inline_code && self.dom.has_markdown_heading_ancestor(n);
            }
            "a" => {
                ctx.link = self.dom.attr(n, "href").to_owned();
                ctx.link_title = self.dom.attr(n, "title").to_owned();
                ctx.role = ColorRole::Accent;
            }
            "del" | "s" | "strike" => ctx.line_through = true,
            "blockquote" => ctx.role = ColorRole::Muted,
            _ => {}
        }
        ctx
    }

    fn paint_block(
        &mut self,
        n: NodeId,
        x: f64,
        y: f64,
        parent_ctx: &Ctx,
        mut available_width: f64,
    ) -> f64 {
        let attrs = self.measure(n, parent_ctx);
        let ctx = self.child_context(n, parent_ctx);
        if self.dom.node(n).kind != NodeType::Element {
            return self.paint_inline_nodes(&[n], x, y, parent_ctx, available_width);
        }
        let data = self.dom.node(n).data.clone();
        match data.as_str() {
            "hr" => {
                let width = if available_width <= 0.0 {
                    attrs.width
                } else {
                    available_width
                };
                let (x1, x2) = (snap(x), snap(x + width));
                let (y1, y2) = (snap(y), snap(y + HEIGHT_HR_EM * ctx.css_font_size()));
                self.primitives
                    .push(Primitive::rect(x1, y1, x2 - x1, y2 - y1, ColorRole::Border));
                return attrs.height;
            }
            "pre" => return self.paint_pre(n, x, y, &attrs, &ctx, available_width),
            "table" => return self.paint_table(n, x, y, &attrs, &ctx, available_width),
            _ => {}
        }
        let (mut content_x, content_y) = (x, y);
        let mut blockquote_border_width = 0.0;
        match data.as_str() {
            "blockquote" => {
                blockquote_border_width = go_max(
                    1.0,
                    (BORDER_LEFT_BLOCKQUOTE_EM * ctx.css_font_size()).floor(),
                );
                let inset =
                    blockquote_border_width + PADDING_LR_BLOCKQUOTE_EM * ctx.css_font_size();
                content_x += inset;
                available_width = go_max(
                    0.0,
                    available_width - inset - PADDING_LR_BLOCKQUOTE_EM * ctx.css_font_size(),
                );
            }
            "li" => {
                let inset = PADDING_LEFT_UL_OL_EM * ctx.css_font_size();
                content_x += inset;
                available_width = go_max(0.0, available_width - inset);
                self.paint_list_marker(n, x, y, &ctx, inset);
            }
            _ => {}
        }
        let content_height = self.paint_flow(n, content_x, content_y, &ctx, available_width);
        let mut painted_height = go_max(attrs.height, content_height);
        if data == "h1" || data == "h2" {
            let line_height = ctx.css_font_size() * ctx.line_height_factor;
            if content_height > line_height {
                painted_height = attrs.height + content_height - line_height;
            }
        }
        if data == "blockquote" {
            let (y1, y2) = (snap(y), snap(y + painted_height));
            self.primitives.push(Primitive::rect(
                snap(x),
                y1,
                blockquote_border_width,
                y2 - y1,
                ColorRole::Border,
            ));
        }
        if data == "h1" || data == "h2" {
            let width = if available_width <= 0.0 {
                attrs.width
            } else {
                available_width
            };
            let (x1, x2) = (snap(x), snap(x + width));
            let raw_top = y + painted_height - BORDER_BOTTOM_H1_H2;
            let (y1, y2) = (snap(raw_top), snap(raw_top + BORDER_BOTTOM_H1_H2));
            self.primitives.push(Primitive::rect(
                x1,
                y1,
                x2 - x1,
                y2 - y1,
                ColorRole::BorderMuted,
            ));
        }
        painted_height
    }

    fn paint_flow(&mut self, n: NodeId, x: f64, y: f64, ctx: &Ctx, available_width: f64) -> f64 {
        let units = self.flow_units(n, ctx);
        let mut current_y = y;
        let mut previous_margin_bottom = 0.0;
        let body = self.dom.is_element(n, "body");
        for (i, unit) in units.iter().enumerate() {
            if i == 0
                && body
                && unit
                    .block
                    .is_some_and(|b| self.dom.is_element(b, "ul") || self.dom.is_element(b, "ol"))
            {
                current_y += unit.attrs.margin_top;
            } else if i > 0 {
                current_y += go_max(previous_margin_bottom, unit.attrs.margin_top);
            }
            let mut painted_height = unit.attrs.height;
            if let Some(block) = unit.block {
                painted_height = self.paint_block(block, x, current_y, ctx, available_width);
            } else {
                painted_height = go_max(
                    painted_height,
                    self.paint_inline_nodes(&unit.inline, x, current_y, ctx, available_width),
                );
            }
            current_y += painted_height;
            previous_margin_bottom = unit.attrs.margin_bottom;
        }
        current_y - y
    }

    fn flow_units(&mut self, n: NodeId, ctx: &Ctx) -> Vec<FlowUnit> {
        let mut units = Vec::new();
        let mut inline_nodes: Vec<NodeId> = Vec::new();
        let mut inline_attrs = BlockAttrs::default();
        let line_height = ctx.css_font_size() * ctx.line_height_factor;
        let flush = |units: &mut Vec<FlowUnit>,
                     inline_nodes: &mut Vec<NodeId>,
                     inline_attrs: &mut BlockAttrs| {
            if inline_nodes.is_empty() || !inline_attrs.is_not_empty() {
                inline_nodes.clear();
                *inline_attrs = BlockAttrs::default();
                return;
            }
            let mut attrs = std::mem::take(inline_attrs);
            attrs.height = line_height;
            units.push(FlowUnit {
                block: None,
                inline: std::mem::take(inline_nodes),
                attrs,
            });
        };
        let dom = self.dom;
        let first = dom.get_next(dom.node(n).first_child);
        let last = dom.get_prev(dom.node(n).last_child);
        let blockquote = dom.is_element(n, "blockquote");
        for child in dom.children(n) {
            if is_block_node(dom, child) {
                flush(&mut units, &mut inline_nodes, &mut inline_attrs);
                let mut attrs = self.measure(child, ctx);
                if Some(child) == first && blockquote {
                    attrs.margin_top = 0.0;
                }
                if Some(child) == last && blockquote {
                    attrs.margin_bottom = 0.0;
                }
                units.push(FlowUnit {
                    block: Some(child),
                    inline: Vec::new(),
                    attrs,
                });
                continue;
            }
            if dom.is_element(child, "br") {
                if inline_attrs.is_not_empty() {
                    flush(&mut units, &mut inline_nodes, &mut inline_attrs);
                } else {
                    units.push(FlowUnit {
                        block: None,
                        inline: Vec::new(),
                        attrs: BlockAttrs::height(line_height),
                    });
                }
                continue;
            }
            let attrs = self.measure(child, ctx);
            inline_nodes.push(child);
            if attrs.is_not_empty() {
                inline_attrs.width += attrs.width;
                inline_attrs.height = go_max(inline_attrs.height, attrs.height);
                inline_attrs.margin_top = go_max(inline_attrs.margin_top, attrs.margin_top);
                inline_attrs.margin_bottom =
                    go_max(inline_attrs.margin_bottom, attrs.margin_bottom);
            }
        }
        flush(&mut units, &mut inline_nodes, &mut inline_attrs);
        units
    }

    fn paint_inline_nodes(
        &mut self,
        nodes: &[NodeId],
        x: f64,
        y: f64,
        ctx: &Ctx,
        available_width: f64,
    ) -> f64 {
        let mut tokens = Vec::new();
        let (mut next_code_group, mut next_decoration_group) = (0, 0);
        for &n in nodes {
            self.collect_inline_tokens(
                n,
                ctx,
                &mut tokens,
                &mut next_code_group,
                &mut next_decoration_group,
                0,
                0.0,
            );
        }
        if tokens.is_empty() {
            return 0.0;
        }
        let lines = self.wrap_inline_tokens(tokens, available_width);
        let mut painted_height = 0.0;
        for line in &lines {
            let line_height = self.markdown_inline_line_height(line, ctx);
            let line_y = y + painted_height;
            let mut line_x = x;
            if !ctx.text_align.is_empty() && available_width > 0.0 {
                let mut line_width = 0.0;
                for token in line {
                    line_width += token.width;
                }
                let extra = go_max(0.0, available_width - line_width);
                if ctx.text_align == "center" {
                    line_x += extra / 2.0;
                } else if ctx.text_align == "right" {
                    line_x += extra;
                }
            }
            let mut cursor_x = line_x;
            let mut code_spans: BTreeMap<i32, [f64; 3]> = BTreeMap::new();
            let mut code_shifts: BTreeMap<i32, f64> = BTreeMap::new();
            let mut code_links: BTreeMap<i32, (String, String)> = BTreeMap::new();
            let mut decoration_spans: BTreeMap<i32, Primitive> = BTreeMap::new();
            for token in line {
                if token.code_group > 0 {
                    let mut span = match code_spans.get(&token.code_group) {
                        Some(span) => *span,
                        None => {
                            let shift = self.inline_code_vertical_shift(
                                ctx,
                                token.ctx(),
                                line_height,
                                token.code_height,
                            );
                            code_shifts.insert(token.code_group, shift);
                            code_links.insert(
                                token.code_group,
                                (token.ctx().link.clone(), token.ctx().link_title.clone()),
                            );
                            [cursor_x, cursor_x, token.code_height]
                        }
                    };
                    span[1] = cursor_x + token.width;
                    span[2] = go_max(span[2], token.code_height);
                    code_spans.insert(token.code_group, span);
                }
                if token.decoration_group > 0 {
                    let mut span = decoration_spans
                        .remove(&token.decoration_group)
                        .unwrap_or_else(|| {
                            Primitive::rect(
                                cursor_x,
                                snap(line_y + line_height / 2.0),
                                0.0,
                                1.0,
                                token.ctx().decoration_role,
                            )
                        });
                    span.width = cursor_x + token.width - span.x;
                    decoration_spans.insert(token.decoration_group, span);
                }
                cursor_x += token.width;
            }
            for (group, span) in &code_spans {
                let height = span[2];
                let mut rect = Primitive::rect(
                    span[0],
                    line_y + (line_height - height) / 2.0 + code_shifts[group],
                    span[1] - span[0],
                    height,
                    ColorRole::NeutralMuted,
                );
                rect.radius = 6.0;
                let (link, title) = &code_links[group];
                rect.link.clone_from(link);
                rect.link_title.clone_from(title);
                self.primitives.push(rect);
            }
            cursor_x = line_x;
            for token in line {
                if token.padding {
                    cursor_x += token.width;
                    continue;
                }
                let tctx = token.ctx();
                let mut font_size = tctx.css_font_size();
                let mut font_scale = tctx.font_scale;
                if tctx.inline_code && !tctx.heading_code {
                    font_scale *= FONT_SIZE_PRE_CODE_EM;
                    font_size *= FONT_SIZE_PRE_CODE_EM;
                }
                let mut baseline_y = self.text_baseline(tctx, line_y, line_height, font_scale);
                if tctx.inline_code && token.code_height > 0.0 {
                    let code_top = line_y + (line_height - token.code_height) / 2.0;
                    let vertical_padding = if !tctx.heading_code {
                        PADDING_TOP_BOTTOM_CODE_EM * font_size
                    } else {
                        0.0
                    };
                    let content_top = code_top + vertical_padding;
                    let content_height = go_max(0.0, token.code_height - 2.0 * vertical_padding);
                    baseline_y = self.text_baseline(tctx, content_top, content_height, font_scale);
                    baseline_y += code_shifts.get(&token.code_group).copied().unwrap_or(0.0);
                }
                baseline_y -= heading_ink_correction(tctx);
                let mut primitive = Primitive::text(
                    cursor_x,
                    baseline_y,
                    token.width,
                    token.height,
                    token.text.clone(),
                    tctx,
                    font_size,
                );
                primitive.link.clone_from(&tctx.link);
                primitive.link_title.clone_from(&tctx.link_title);
                primitive.text_length = token.discretionary_hyphen;
                self.primitives.push(primitive);
                cursor_x += token.width;
            }
            self.primitives.extend(decoration_spans.into_values());
            painted_height += line_height;
        }
        painted_height
    }

    fn markdown_inline_line_height(&mut self, line: &[Token], ctx: &Ctx) -> f64 {
        let mut line_height = ctx.css_font_size() * ctx.line_height_factor;
        if !ctx.table_cell {
            return line_height;
        }
        let mut line_ctx = ctx.clone();
        let mut has_content = false;
        let mut all_code = true;
        let mut has_fallback = false;
        let mut has_symbol_fallback = false;
        let mut has_color_emoji_fallback = false;
        for token in line {
            if token.padding || token.text.is_empty() {
                continue;
            }
            has_content = true;
            let tctx = token.ctx();
            if !tctx.inline_code {
                let font = tctx.paint_font();
                let measurement_text = token.text.replace('\u{ad}', "");
                if self
                    .ruler
                    .measure_font_advance(font, &measurement_text)
                    .is_none()
                {
                    for (grapheme, _) in ruler::graphemes(&measurement_text) {
                        if self.ruler.measure_font_advance(font, grapheme).is_some() {
                            continue;
                        }
                        let mut grapheme = grapheme.to_owned();
                        let color_emoji = uniseg::is_color_emoji(&grapheme);
                        if !color_emoji {
                            let filtered = ruler::strip_default_ignorable_runes(&grapheme);
                            if filtered != grapheme {
                                if filtered.is_empty() {
                                    continue;
                                }
                                if self.ruler.measure_font_advance(font, &filtered).is_some() {
                                    continue;
                                }
                                grapheme = filtered;
                            }
                        }
                        has_fallback = true;
                        if color_emoji {
                            has_color_emoji_fallback = true;
                        } else if ruler::has_unicode_symbol(&grapheme) {
                            has_symbol_fallback = true;
                        }
                    }
                }
                all_code = false;
                continue;
            }
            if !line_ctx.inline_code {
                line_ctx = tctx.clone();
            }
        }
        all_code = has_content && all_code;
        if !all_code {
            line_ctx = ctx.clone();
        }
        let mut font_size = line_ctx.css_font_size();
        if all_code {
            font_size *= FONT_SIZE_PRE_CODE_EM;
        }
        if let Some(normal) = self
            .ruler
            .css_normal_line_height(line_ctx.paint_font(), font_size)
        {
            line_height = normal;
        }
        if all_code {
            line_height = go_max(line_height, (font_size * 1.4).round());
        }
        if has_color_emoji_fallback && !all_code {
            line_height = go_max(line_height, (ctx.css_font_size() * 1.625).round());
        } else if has_symbol_fallback && !all_code {
            line_height = go_max(line_height, (ctx.css_font_size() * 1.5).round());
        } else if has_fallback && !all_code {
            line_height = go_max(line_height, (ctx.css_font_size() * 1.375).round());
        }
        line_height
    }

    fn wrap_inline_tokens(&mut self, tokens: Vec<Token>, available_width: f64) -> Vec<Vec<Token>> {
        let mut lines: Vec<Vec<Token>> = vec![Vec::new()];
        let mut line_width = 0.0;
        let mut needs_wrap = false;
        for token in &tokens {
            if token.break_line {
                lines.push(Vec::new());
                line_width = 0.0;
                continue;
            }
            lines.last_mut().expect("line").push(token.clone());
            line_width += token.width;
            needs_wrap =
                needs_wrap || available_width > 0.0 && line_width > available_width + 0.001;
        }
        if !needs_wrap {
            return lines;
        }
        let mut atoms = self.atomize_inline_tokens(&tokens);
        lines = vec![Vec::new()];
        line_width = 0.0;
        let mut pending_space: Option<Token> = None;
        let mut pending_padding: Vec<Token> = Vec::new();
        let mut pending_padding_width = 0.0;
        let mut line_has_content = false;
        let mut atom_index = 0;
        while atom_index < atoms.len() {
            let mut atom = atoms[atom_index].clone();
            if atom.break_line {
                pending_space = None;
                pending_padding.clear();
                pending_padding_width = 0.0;
                lines.push(Vec::new());
                line_width = 0.0;
                line_has_content = false;
                atom_index += 1;
                continue;
            }
            if atom.text == " " && !atom.padding {
                pending_space = Some(atom);
                atom_index += 1;
                continue;
            }
            if atom.padding && atom.leading_padding {
                pending_padding_width += atom.width;
                pending_padding.push(atom);
                atom_index += 1;
                continue;
            }
            loop {
                let trailing_padding_width = inline_trailing_padding_width(&atoms, atom_index);
                let mut needed = inline_unbroken_width(&atoms, atom_index) + pending_padding_width;
                if let Some(space) = &pending_space {
                    needed += space.width;
                }
                let can_wrap = atom.break_before || pending_space.is_some();
                if available_width > 0.0
                    && line_has_content
                    && !atom.padding
                    && can_wrap
                    && line_width + needed > available_width + 0.001
                {
                    if atom.break_after_soft_hyphen {
                        let line = lines.last_mut().expect("line");
                        for i in (0..line.len()).rev() {
                            if !line[i].soft_hyphen_end {
                                continue;
                            }
                            line[i].soft_hyphen_end = false;
                            let mut hyphen = line[i].clone();
                            hyphen.text = "-".into();
                            hyphen.width = hyphen.ctx().css_font_size() / 3.0;
                            hyphen.soft_hyphen_end = false;
                            hyphen.discretionary_hyphen = true;
                            hyphen.break_before = false;
                            hyphen.break_after_soft_hyphen = false;
                            // Go's `lineWidth += hyphen.width` is dead: the
                            // line ends immediately below.
                            line.insert(i + 1, hyphen);
                            break;
                        }
                    }
                    lines.push(Vec::new());
                    line_width = 0.0;
                    line_has_content = false;
                    pending_space = None;
                }
                let mut occupied_width = line_width + pending_padding_width;
                if let Some(space) = &pending_space {
                    if line_has_content {
                        occupied_width += space.width;
                    }
                }
                if available_width > 0.0
                    && !atom.padding
                    && occupied_width + atom.width + trailing_padding_width
                        > available_width + 0.001
                {
                    let max_text_width = go_max(0.0, available_width - occupied_width);
                    let split = self.split_inline_token_at_width(
                        &atom,
                        max_text_width,
                        trailing_padding_width > 0.0,
                    );
                    if split.is_none() && line_has_content {
                        lines.push(Vec::new());
                        line_width = 0.0;
                        line_has_content = false;
                        pending_space = None;
                        continue;
                    }
                    if let Some((head, tail)) = split {
                        atom = head;
                        atoms.insert(atom_index + 1, tail);
                    }
                }
                break;
            }
            if let Some(space) = pending_space.take() {
                if line_has_content {
                    line_width += space.width;
                    lines.last_mut().expect("line").push(space);
                }
            }
            lines.last_mut().expect("line").append(&mut pending_padding);
            line_width += pending_padding_width;
            pending_padding_width = 0.0;
            line_width += atom.width;
            if !atom.padding {
                line_has_content = true;
            }
            lines.last_mut().expect("line").push(atom);
            atom_index += 1;
        }
        lines
    }

    fn split_inline_token_at_width(
        &mut self,
        token: &Token,
        max_width: f64,
        force_split: bool,
    ) -> Option<(Token, Token)> {
        let boundaries = uniseg::grapheme_boundaries(&token.text);
        if boundaries.len() < 2 {
            return None;
        }
        let mut best: isize = -1;
        for (i, &boundary) in boundaries.iter().enumerate() {
            let width = self.inline_text_advance(&token.text[..boundary], token.ctx(), token.width);
            if width > max_width + 0.001 {
                break;
            }
            best = i as isize;
        }
        if best < 0 {
            return None;
        }
        if best as usize == boundaries.len() - 1 {
            if !force_split {
                return None;
            }
            best -= 1;
            if best < 0 {
                return None;
            }
        }
        let boundary = boundaries[best as usize];
        let mut head = token.clone();
        head.text = token.text[..boundary].to_owned();
        head.width = self.inline_text_advance(&head.text, token.ctx(), token.width);
        head.soft_hyphen_end = false;
        let mut tail = token.clone();
        tail.text = token.text[boundary..].to_owned();
        tail.width = self.inline_text_advance(&tail.text, token.ctx(), token.width);
        tail.break_before = false;
        tail.break_after_soft_hyphen = false;
        Some((head, tail))
    }

    fn atomize_inline_tokens(&mut self, tokens: &[Token]) -> Vec<Token> {
        let mut atoms = Vec::new();
        let mut start = 0;
        while start < tokens.len() {
            if tokens[start].break_line {
                atoms.push(tokens[start].clone());
                start += 1;
                continue;
            }
            let mut end = start;
            let mut combined = String::new();
            while end < tokens.len() && !tokens[end].break_line {
                if !tokens[end].padding {
                    combined.push_str(&tokens[end].text);
                }
                end += 1;
            }
            let mut grapheme_boundaries = std::collections::HashSet::from([0usize]);
            grapheme_boundaries.extend(uniseg::grapheme_boundaries(&combined));
            let break_offsets: std::collections::HashSet<usize> =
                uniseg::line_boundaries(&combined)
                    .into_iter()
                    .map(|(offset, _)| offset)
                    .filter(|&offset| {
                        offset < combined.len() && grapheme_boundaries.contains(&offset)
                    })
                    .collect();
            let mut text_offset = 0;
            for token in &tokens[start..end] {
                if token.padding || token.text.is_empty() {
                    atoms.push(token.clone());
                    continue;
                }
                let token_start = text_offset;
                let token_end = token_start + token.text.len();
                let mut piece_start = token_start;
                for offset in token_start + 1..=token_end {
                    if offset != token_end && !break_offsets.contains(&offset) {
                        continue;
                    }
                    let mut piece = &token.text[piece_start - token_start..offset - token_start];
                    let mut atom = token.clone();
                    atom.break_before = break_offsets.contains(&piece_start);
                    atom.break_after_soft_hyphen =
                        atom.break_before && combined[..piece_start].ends_with('\u{ad}');
                    if let Some(rest) = piece.strip_suffix('\u{ad}') {
                        piece = rest;
                        atom.soft_hyphen_end = true;
                    }
                    self.append_inline_text_atoms(&mut atoms, atom, piece);
                    piece_start = offset;
                }
                text_offset = token_end;
            }
            start = end;
        }
        atoms
    }

    fn append_inline_text_atoms(&mut self, atoms: &mut Vec<Token>, mut token: Token, text: &str) {
        if text.is_empty() && token.soft_hyphen_end {
            token.width = 0.0;
            atoms.push(token);
            return;
        }
        let content = text.trim_end_matches(' ');
        if !content.is_empty() {
            let mut atom = token.clone();
            atom.text = content.to_owned();
            atom.width = self.inline_text_advance(content, token.ctx(), token.width);
            atoms.push(atom);
            token.break_before = false;
        }
        for _ in 0..text.len() - content.len() {
            let mut atom = token.clone();
            atom.text = " ".into();
            atom.width = self.inline_text_advance(" ", token.ctx(), token.width);
            atoms.push(atom);
            token.break_before = false;
        }
    }

    fn inline_code_vertical_shift(
        &mut self,
        parent_ctx: &Ctx,
        code_ctx: &Ctx,
        line_height: f64,
        code_height: f64,
    ) -> f64 {
        if parent_ctx.table_cell {
            return 0.0;
        }
        let mut code_scale = code_ctx.font_scale;
        let mut code_font_size = code_ctx.css_font_size();
        let mut vertical_padding = 0.0;
        if !code_ctx.heading_code {
            code_scale *= FONT_SIZE_PRE_CODE_EM;
            code_font_size *= FONT_SIZE_PRE_CODE_EM;
            vertical_padding = PADDING_TOP_BOTTOM_CODE_EM * code_font_size;
        }
        let code_top = (line_height - code_height) / 2.0 + vertical_padding;
        let code_content_height = go_max(0.0, code_height - 2.0 * vertical_padding);
        let code_baseline = self.text_baseline(code_ctx, code_top, code_content_height, code_scale);
        let parent_baseline =
            self.text_baseline(parent_ctx, 0.0, line_height, parent_ctx.font_scale);
        parent_baseline - code_baseline
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_inline_tokens(
        &mut self,
        n: NodeId,
        ctx: &Ctx,
        tokens: &mut Vec<Token>,
        next_code_group: &mut i32,
        next_decoration_group: &mut i32,
        mut code_group: i32,
        mut code_height: f64,
    ) {
        let node = self.dom.node(n);
        if node.kind == NodeType::Text {
            let text = node.data.clone();
            let attrs = self.measure(n, ctx);
            if text.is_empty() && attrs.width == 0.0 {
                return;
            }
            let width = self.inline_text_advance(&text, ctx, attrs.width);
            tokens.push(Token {
                text,
                width,
                height: attrs.height,
                ctx: Some(ctx.clone()),
                code_group,
                code_height,
                decoration_group: ctx.decoration_group,
                ..Default::default()
            });
            return;
        }
        if node.kind != NodeType::Element {
            return;
        }
        if node.data == "br" {
            if !ctx.inline_code {
                tokens.push(Token {
                    break_line: true,
                    ..Default::default()
                });
            }
            return;
        }
        if node.data == "img" {
            return;
        }
        let mut child_ctx = self.child_context(n, ctx);
        if child_ctx.line_through && !ctx.line_through {
            *next_decoration_group += 1;
            child_ctx.decoration_group = *next_decoration_group;
            child_ctx.decoration_role = child_ctx.role;
        }
        let inline_code =
            node.data == "code" && node.parent.is_none_or(|p| self.dom.node(p).data != "pre");
        if inline_code {
            *next_code_group += 1;
            code_group = *next_code_group;
            code_height = self.measure(n, ctx).height;
            tokens.push(Token {
                padding: true,
                leading_padding: true,
                width: inline_code_horizontal_padding(&child_ctx),
                ctx: Some(child_ctx.clone()),
                code_group,
                code_height,
                decoration_group: child_ctx.decoration_group,
                ..Default::default()
            });
        }
        let children: Vec<_> = self.dom.children(n).collect();
        for child in children {
            self.collect_inline_tokens(
                child,
                &child_ctx,
                tokens,
                next_code_group,
                next_decoration_group,
                code_group,
                code_height,
            );
        }
        if inline_code {
            tokens.push(Token {
                padding: true,
                width: inline_code_horizontal_padding(&child_ctx),
                ctx: Some(child_ctx.clone()),
                code_group,
                code_height,
                decoration_group: child_ctx.decoration_group,
                ..Default::default()
            });
        }
    }

    fn inline_text_advance(&mut self, text: &str, ctx: &Ctx, fallback: f64) -> f64 {
        let font = ctx.paint_font();
        let measurement_text = text.replace('\u{ad}', "");
        let mut advance = match self.ruler.measure_font_advance(font, &measurement_text) {
            Some(advance) => advance,
            None => {
                let (w, _) = self.ruler.measure_precise(font, &measurement_text);
                let advance = self.ruler.scale_unicode_css(w, font, &measurement_text);
                if advance == 0.0
                    && !measurement_text.is_empty()
                    && !ruler::is_zero_width_grapheme(&measurement_text)
                {
                    return fallback;
                }
                advance
            }
        };
        advance *= ctx.font_scale;
        if ctx.inline_code && !ctx.heading_code {
            advance *= FONT_SIZE_PRE_CODE_EM;
        }
        advance
    }

    fn paint_pre(
        &mut self,
        n: NodeId,
        x: f64,
        y: f64,
        attrs: &BlockAttrs,
        ctx: &Ctx,
        available_width: f64,
    ) -> f64 {
        let background_width = go_max(attrs.width, available_width);
        let mut background = Primitive::rect(
            x,
            y,
            background_width,
            attrs.height,
            ColorRole::CanvasSubtle,
        );
        background.radius = 6.0;
        let text = self.dom.markdown_node_text(n);
        let mut lines: Vec<&str> = if text.is_empty() {
            Vec::new()
        } else {
            text.split('\n').collect()
        };
        if lines.last().is_some_and(|l| l.is_empty()) && text.ends_with('\n') {
            lines.pop();
        }
        let font_size = FONT_SIZE_PRE_CODE_EM * ctx.css_font_size();
        let line_height = LINE_HEIGHT_PRE * font_size;
        let background_height = if lines.is_empty() {
            2.0 * PADDING_PRE
        } else {
            lines.len() as f64 * line_height + 2.0 * PADDING_PRE
        };
        background.height = background_height;
        self.primitives.push(background);
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let line = expand_markdown_tabs(line);
            let baseline = self.text_baseline(
                ctx,
                y + PADDING_PRE + i as f64 * line_height,
                line_height,
                FONT_SIZE_PRE_CODE_EM * ctx.font_scale,
            );
            self.primitives.push(Primitive::text(
                x + PADDING_PRE,
                baseline,
                0.0,
                0.0,
                line,
                ctx,
                font_size,
            ));
        }
        background_height
    }

    fn paint_table(
        &mut self,
        n: NodeId,
        x: f64,
        y: f64,
        attrs: &BlockAttrs,
        ctx: &Ctx,
        available_width: f64,
    ) -> f64 {
        let mut rows = collect_markdown_table_rows(self.dom, n);
        let mut column_widths = Vec::new();
        let mut column_min_widths = Vec::new();
        for row in rows.iter_mut() {
            let mut row_min_widths = Vec::new();
            for &cell in &row.cells.clone() {
                let cell_ctx = table_cell_context(self.dom, ctx, row, cell);
                let cell_attrs = self.measure(cell, &cell_ctx);
                row.widths.push(cell_attrs.width + 26.0);
                row_min_widths.push(self.markdown_min_content_width(cell, &cell_ctx) + 26.0);
            }
            column_widths = merge_column_widths(column_widths, std::slice::from_ref(&row.widths));
            column_min_widths = merge_column_widths(column_min_widths, &[row_min_widths]);
        }
        let mut table_width = attrs.width;
        if available_width > 0.0 {
            table_width = table_width.min(available_width);
        }
        table_width = go_max(1.0, table_width);
        let column_widths =
            fit_markdown_table_columns(&column_widths, &column_min_widths, table_width);
        let mut painted_table_width = (column_widths.len() + 1) as f64;
        for w in &column_widths {
            painted_table_width += w;
        }
        painted_table_width = go_max(table_width, painted_table_width);

        for row in rows.iter_mut() {
            let mut max_cell_height: f64 = 0.0;
            for (cell_index, &cell) in row.cells.clone().iter().enumerate() {
                let cell_ctx = table_cell_context(self.dom, ctx, row, cell);
                let content_width = go_max(0.0, column_widths[cell_index] - 26.0);
                let saved = std::mem::take(&mut self.primitives);
                let content_height = self.paint_flow(cell, 0.0, 0.0, &cell_ctx, content_width);
                self.primitives = saved;
                row.heights.push(content_height);
                max_cell_height = go_max(max_cell_height, content_height + 12.0);
            }
            row.height = max_cell_height + 1.0;
        }
        let mut final_boundary_y = y;
        for row in &rows {
            final_boundary_y += row.height;
        }
        let left_border_x = snap(x);
        let top_border_y = snap(y);
        let right_border_x = snap(x + painted_table_width - 1.0);
        let bottom_border_y = snap(final_boundary_y);
        let grid_width = right_border_x - left_border_x + 1.0;
        let grid_height = bottom_border_y - top_border_y + 1.0;
        self.primitives.push(Primitive::rect(
            left_border_x,
            top_border_y,
            grid_width,
            grid_height,
            ColorRole::Canvas,
        ));
        let mut stripe_y = y;
        for row in &rows {
            if row.stripe {
                let (top, bottom) = (snap(stripe_y), snap(stripe_y + row.height));
                self.primitives.push(Primitive::rect(
                    left_border_x + 1.0,
                    top,
                    go_max(0.0, right_border_x - left_border_x - 1.0),
                    bottom - top,
                    ColorRole::CanvasSubtle,
                ));
            }
            stripe_y += row.height;
        }
        let mut grid = vec![
            Primitive::rect(
                left_border_x,
                top_border_y,
                grid_width,
                1.0,
                ColorRole::Border,
            ),
            Primitive::rect(
                left_border_x,
                top_border_y,
                1.0,
                grid_height,
                ColorRole::Border,
            ),
            Primitive::rect(
                right_border_x,
                top_border_y,
                1.0,
                grid_height,
                ColorRole::Border,
            ),
        ];
        let mut row_y = y;
        for row in &rows {
            let mut cursor_x = x + 1.0;
            for (cell_index, &cell) in row.cells.iter().enumerate() {
                let cell_ctx = table_cell_context(self.dom, ctx, row, cell);
                let content_x = cursor_x + 13.0;
                let available = column_widths[cell_index] - 26.0;
                let content_height = row.heights.get(cell_index).copied().unwrap_or(0.0);
                let content_y = row_y + 7.0 + go_max(0.0, row.height - 13.0 - content_height) / 2.0;
                self.paint_flow(cell, content_x, content_y, &cell_ctx, available);
                cursor_x += column_widths[cell_index];
                if cell_index < row.cells.len() - 1 {
                    let (top, bottom) = (snap(row_y), snap(row_y + row.height));
                    grid.push(Primitive::rect(
                        snap(cursor_x),
                        top,
                        1.0,
                        bottom - top,
                        ColorRole::Border,
                    ));
                }
                cursor_x += 1.0;
            }
            row_y += row.height;
            grid.push(Primitive::rect(
                left_border_x,
                snap(row_y),
                grid_width,
                1.0,
                ColorRole::Border,
            ));
        }
        self.primitives.extend(grid);
        grid_height
    }

    fn markdown_min_content_width(&mut self, n: NodeId, ctx: &Ctx) -> f64 {
        let mut min_width: f64 = 0.0;
        for unit in self.flow_units(n, ctx) {
            if let Some(block) = unit.block {
                min_width = go_max(min_width, self.measure(block, ctx).width);
                continue;
            }
            let mut tokens = Vec::new();
            let (mut next_code_group, mut next_decoration_group) = (0, 0);
            for &inline in &unit.inline {
                self.collect_inline_tokens(
                    inline,
                    ctx,
                    &mut tokens,
                    &mut next_code_group,
                    &mut next_decoration_group,
                    0,
                    0.0,
                );
            }
            let atoms = self.atomize_inline_tokens(&tokens);
            let mut segment_width = 0.0;
            let mut pending_leading_padding = 0.0;
            let mut previous_content_ctx: Option<Ctx> = None;
            let commit = |min_width: &mut f64,
                          segment_width: &mut f64,
                          previous: &mut Option<Ctx>,
                          extra: f64| {
                *min_width = go_max(*min_width, *segment_width + extra);
                *segment_width = 0.0;
                *previous = None;
            };
            for atom in &atoms {
                if atom.break_line || (atom.text == " " && !atom.padding) {
                    segment_width += pending_leading_padding;
                    pending_leading_padding = 0.0;
                    commit(
                        &mut min_width,
                        &mut segment_width,
                        &mut previous_content_ctx,
                        0.0,
                    );
                    continue;
                }
                if atom.padding && atom.leading_padding {
                    pending_leading_padding += atom.width;
                    continue;
                }
                if !atom.padding && atom.break_before {
                    let mut extra = 0.0;
                    if atom.break_after_soft_hyphen {
                        if let Some(previous) = &previous_content_ctx {
                            extra = previous.css_font_size() / 3.0;
                        }
                    }
                    commit(
                        &mut min_width,
                        &mut segment_width,
                        &mut previous_content_ctx,
                        extra,
                    );
                }
                segment_width += pending_leading_padding;
                pending_leading_padding = 0.0;
                segment_width += atom.width;
                if !atom.padding {
                    previous_content_ctx = atom.ctx.clone();
                }
            }
            segment_width += pending_leading_padding;
            commit(
                &mut min_width,
                &mut segment_width,
                &mut previous_content_ctx,
                0.0,
            );
        }
        min_width
    }

    fn paint_list_marker(&mut self, n: NodeId, x: f64, y: f64, ctx: &Ctx, inset: f64) {
        let baseline_y = self.list_marker_baseline(n, y, ctx);
        let parent = self.dom.node(n).parent;
        if parent.is_some_and(|p| self.dom.is_element(p, "ul")) {
            let font_size = ctx.css_font_size();
            let side = go_max(3.0, snap(0.3 * font_size));
            let center_x = x + inset - font_size / 2.0 - 6.5;
            let mut primitive = Primitive::rect(
                snap(center_x - side / 2.0),
                baseline_y - (1.5 * side).floor(),
                side,
                side,
                ColorRole::None,
            );
            match markdown_list_depth(self.dom, n) {
                0 => {
                    primitive.radius = side / 2.0;
                    primitive.fill_role = ctx.role;
                }
                1 => {
                    primitive.radius = side / 2.0;
                    primitive.stroke_width = 1.0;
                    primitive.stroke_role = if ctx.role == ColorRole::Muted {
                        ColorRole::MutedStroke
                    } else {
                        ColorRole::ForegroundStroke
                    };
                }
                _ => primitive.fill_role = ctx.role,
            }
            self.primitives.push(primitive);
            return;
        }
        let marker = markdown_list_marker(self.dom, n);
        if marker.is_empty() {
            return;
        }
        let font = ctx.paint_font();
        let (mut marker_width, mut marker_height) = self.ruler.measure_precise(font, &marker);
        marker_width *= ctx.font_scale;
        marker_height *= ctx.font_scale;
        let space = self.ruler.space_width(font) * ctx.font_scale;
        self.primitives.push(Primitive::text(
            x + inset - marker_width - space,
            baseline_y,
            marker_width,
            marker_height,
            marker,
            ctx,
            ctx.css_font_size(),
        ));
    }

    fn list_marker_baseline(&mut self, n: NodeId, y: f64, ctx: &Ctx) -> f64 {
        let baseline = self.text_baseline(
            ctx,
            y,
            ctx.css_font_size() * ctx.line_height_factor,
            ctx.font_scale,
        );
        let first = self.dom.get_next(self.dom.node(n).first_child);
        let Some(first) =
            first.filter(|&f| self.dom.element_data(f).is_some_and(is_markdown_heading))
        else {
            return baseline;
        };
        let heading_ctx = self.child_context(first, ctx);
        let heading_baseline = self.text_baseline(
            &heading_ctx,
            y,
            heading_ctx.css_font_size() * heading_ctx.line_height_factor,
            heading_ctx.font_scale,
        );
        go_max(baseline, heading_baseline)
    }

    /// textBaseline.
    fn text_baseline(&mut self, ctx: &Ctx, top: f64, line_height: f64, scale: f64) -> f64 {
        let font = ctx.paint_font();
        if let Some((ascent, descent)) = self
            .ruler
            .css_font_box_metrics(font, ctx.font_size as f64 * scale)
        {
            let leading = go_max(0.0, line_height - ascent - descent);
            return top + leading / 2.0 + ascent;
        }
        let (atlas_line_height, atlas_ascent) = self.ruler.atlas_line_height_ascent(font);
        let font_height = atlas_line_height * scale;
        let leading = go_max(0.0, line_height - font_height);
        top + leading / 2.0 + atlas_ascent * scale
    }
}

/// markdownHeadingInkCorrection.
fn heading_ink_correction(ctx: &Ctx) -> f64 {
    if ctx.heading == "h5" || (ctx.heading == "h2" && ctx.font_family == FontFamily::SourceSansPro)
    {
        1.0
    } else {
        0.0
    }
}

fn inline_trailing_padding_width(atoms: &[Token], atom_index: usize) -> f64 {
    let mut width = 0.0;
    let mut i = atom_index + 1;
    while i < atoms.len() && atoms[i].padding && !atoms[i].leading_padding {
        width += atoms[i].width;
        i += 1;
    }
    width
}

fn inline_unbroken_width(atoms: &[Token], atom_index: usize) -> f64 {
    let mut width = 0.0;
    for (i, atom) in atoms.iter().enumerate().skip(atom_index) {
        if atom.break_line
            || (i > atom_index && !atom.padding && (atom.break_before || atom.text == " "))
        {
            break;
        }
        if i == atom_index && atom.text == " " && !atom.padding {
            break;
        }
        width += atom.width;
    }
    width
}

fn inline_code_horizontal_padding(ctx: &Ctx) -> f64 {
    if ctx.heading_code {
        return PADDING_LEFT_RIGHT_HEADING_CODE_EM * ctx.css_font_size();
    }
    PADDING_LEFT_RIGHT_CODE_EM * FONT_SIZE_PRE_CODE_EM * ctx.css_font_size()
}

fn expand_markdown_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_owned();
    }
    let mut out = String::new();
    let mut column = 0;
    for (grapheme, width) in ruler::graphemes(line) {
        if grapheme == "\t" {
            let spaces = 4 - column % 4;
            out.push_str(&" ".repeat(spaces));
            column += spaces;
            continue;
        }
        out.push_str(grapheme);
        column += width.max(1);
    }
    out
}

struct TableRow {
    cells: Vec<NodeId>,
    header: bool,
    height: f64,
    widths: Vec<f64>,
    heights: Vec<f64>,
    stripe: bool,
}

fn collect_markdown_table_rows(dom: &Dom, table: NodeId) -> Vec<TableRow> {
    fn append_row(dom: &Dom, rows: &mut Vec<TableRow>, n: NodeId, in_header: bool, stripe: bool) {
        let mut row = TableRow {
            cells: Vec::new(),
            header: in_header,
            height: 0.0,
            widths: Vec::new(),
            heights: Vec::new(),
            stripe,
        };
        for child in dom.children(n) {
            if let Some(cell @ ("td" | "th")) = dom.element_data(child) {
                row.cells.push(child);
                if cell == "th" {
                    row.header = true;
                }
            }
        }
        rows.push(row);
    }
    fn walk(dom: &Dom, rows: &mut Vec<TableRow>, n: NodeId, mut in_header: bool) {
        let Some(data) = dom.element_data(n) else {
            return;
        };
        if matches!(data, "thead" | "tbody" | "tfoot" | "table") {
            if data == "thead" {
                in_header = true;
            }
            let mut row_index = 0;
            for child in dom.children(n) {
                if dom.is_element(child, "tr") {
                    append_row(dom, rows, child, in_header, row_index % 2 == 1);
                    row_index += 1;
                } else {
                    walk(dom, rows, child, in_header);
                }
            }
            return;
        }
        for child in dom.children(n) {
            walk(dom, rows, child, in_header);
        }
    }
    let mut rows = Vec::new();
    walk(dom, &mut rows, table, false);
    rows
}

fn table_cell_context(dom: &Dom, ctx: &Ctx, row: &TableRow, cell: NodeId) -> Ctx {
    let mut ctx = ctx.clone();
    ctx.table_cell = true;
    ctx.line_height_factor = 1.25;
    let th = dom.is_element(cell, "th");
    if row.header || th {
        ctx.font_style = FontStyle::Semibold;
        ctx.bold = true;
    }
    ctx.text_align = dom.attr(cell, "align").to_owned();
    if ctx.text_align.is_empty() && (row.header || th) {
        ctx.text_align = "center".into();
    }
    ctx
}

fn fit_markdown_table_columns(widths: &[f64], min_widths: &[f64], table_width: f64) -> Vec<f64> {
    if widths.is_empty() {
        return widths.to_vec();
    }
    let mut fitted = widths.to_vec();
    let column_budget = go_max(0.0, table_width - (widths.len() + 1) as f64);
    let mut natural_width = 0.0;
    for w in widths {
        natural_width += w;
    }
    if natural_width <= column_budget + 0.001 {
        return fitted;
    }
    let minimum = |i: usize, width: f64| {
        if i < min_widths.len() {
            width.min(min_widths[i])
        } else {
            0.0
        }
    };
    let (mut min_width, mut shrink_capacity) = (0.0, 0.0);
    for (i, &width) in widths.iter().enumerate() {
        let m = minimum(i, width);
        min_width += m;
        shrink_capacity += width - m;
    }
    if min_width <= column_budget + 0.001 && shrink_capacity > 0.0 {
        let shrink = natural_width - column_budget;
        for (i, &width) in widths.iter().enumerate() {
            let capacity = width - minimum(i, width);
            fitted[i] = width - shrink * capacity / shrink_capacity;
        }
        return fitted;
    }
    if min_width > column_budget + 0.001 {
        for (i, &width) in widths.iter().enumerate() {
            if i < min_widths.len() {
                fitted[i] = width.min(min_widths[i]);
            }
        }
        return fitted;
    }
    let padding_width = 26.0 * widths.len() as f64;
    if column_budget <= padding_width {
        let n = fitted.len() as f64;
        for f in fitted.iter_mut() {
            *f = column_budget / n;
        }
        return fitted;
    }
    let content_budget = column_budget - padding_width;
    let mut natural_content_width = 0.0;
    for &w in widths {
        natural_content_width += go_max(0.0, w - 26.0);
    }
    if natural_content_width == 0.0 {
        let n = fitted.len() as f64;
        for f in fitted.iter_mut() {
            *f = 26.0 + content_budget / n;
        }
        return fitted;
    }
    for (i, &width) in widths.iter().enumerate() {
        let share = go_max(0.0, width - 26.0) / natural_content_width;
        fitted[i] = 26.0 + content_budget * share;
    }
    fitted
}

fn markdown_list_marker(dom: &Dom, n: NodeId) -> String {
    let Some(parent) = dom
        .node(n)
        .parent
        .filter(|&p| dom.node(p).kind == NodeType::Element)
    else {
        return String::new();
    };
    if dom.is_element(parent, "ul") || !dom.is_element(parent, "ol") {
        return String::new();
    }
    // strconv.Atoi and i64::from_str accept the same signed decimal forms.
    let mut index: i64 = dom.attr(parent, "start").parse().unwrap_or(1);
    let mut sibling = dom.node(n).prev_sibling;
    while let Some(s) = sibling {
        if dom.is_element(s, "li") {
            index += 1;
        }
        sibling = dom.node(s).prev_sibling;
    }
    let mut style = dom.attr(parent, "type").to_owned();
    if style.is_empty() {
        let mut depth = 0;
        let mut ancestor = dom.node(parent).parent;
        while let Some(a) = ancestor {
            if dom.is_element(a, "ol") || dom.is_element(a, "ul") {
                depth += 1;
            }
            ancestor = dom.node(a).parent;
        }
        style = match depth {
            0 => "1",
            1 => "i",
            _ => "a",
        }
        .into();
    }
    ordered_list_marker(index, &style)
}

fn markdown_list_depth(dom: &Dom, n: NodeId) -> i32 {
    let mut depth = 0;
    let Some(parent) = dom.node(n).parent else {
        return 0;
    };
    let mut ancestor = dom.node(parent).parent;
    while let Some(a) = ancestor {
        if dom.is_element(a, "ol") || dom.is_element(a, "ul") {
            depth += 1;
        }
        ancestor = dom.node(a).parent;
    }
    depth
}

fn ordered_list_marker(index: i64, style: &str) -> String {
    match style.to_lowercase().as_str() {
        "a" => lower_alpha(index) + ".",
        "i" => lower_roman(index) + ".",
        _ => format!("{index}."),
    }
}

fn lower_alpha(mut index: i64) -> String {
    if index <= 0 {
        return index.to_string();
    }
    let mut out = Vec::new();
    while index > 0 {
        index -= 1;
        out.push(b'a' + (index % 26) as u8);
        index /= 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

fn lower_roman(mut index: i64) -> String {
    if index <= 0 || index > 3999 {
        return index.to_string();
    }
    const VALUES: [(i64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, text) in VALUES {
        while index >= value {
            out.push_str(text);
            index -= value;
        }
    }
    out
}

/// validateMarkdownSVGNodes.
fn validate_markdown_svg_nodes(dom: &Dom, n: NodeId) -> Result<(), String> {
    if let Some(data) = dom.element_data(n) {
        if !matches!(
            data,
            "html"
                | "head"
                | "body"
                | "p"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "blockquote"
                | "ul"
                | "ol"
                | "li"
                | "pre"
                | "code"
                | "em"
                | "b"
                | "strong"
                | "del"
                | "s"
                | "strike"
                | "a"
                | "br"
                | "hr"
                | "img"
                | "table"
                | "thead"
                | "tbody"
                | "tfoot"
                | "tr"
                | "td"
                | "th"
        ) {
            return Err(format!(
                "native Markdown SVG does not support HTML element <{data}>"
            ));
        }
    }
    for child in dom.children(n) {
        validate_markdown_svg_nodes(dom, child)?;
    }
    Ok(())
}

/// LayoutMarkdown.
pub(crate) fn layout_markdown(
    md_text: &str,
    ruler: &mut Ruler,
    font_family: FontFamily,
    mono_font_family: FontFamily,
    font_size: i64,
) -> Result<MarkdownLayout, String> {
    let render = d2_rs_text::markdown::html::render_markdown_html(md_text.as_bytes());
    let (legacy_dom, legacy_body) = dom::parse_document(&render);
    let mut dom = legacy_dom.clone();
    let body = legacy_body;
    validate_markdown_svg_nodes(&dom, body)?;
    normalize_markdown_whitespace_tree(&mut dom, body);

    let original_line_height = ruler.line_height_factor;
    let original_bounds_with_dot = ruler.bounds_with_dot;
    ruler.line_height_factor = MARKDOWN_LINE_HEIGHT;
    ruler.bounds_with_dot = true;

    let ctx = Ctx {
        font_family,
        mono_font_family,
        font_size,
        font_scale: 1.0,
        font_style: FontStyle::Regular,
        bold: false,
        italic: false,
        line_height_factor: MARKDOWN_LINE_HEIGHT,
        mono: font_family == mono_font_family,
        inline_code: false,
        heading_code: false,
        heading: String::new(),
        role: ColorRole::Foreground,
        link: String::new(),
        link_title: String::new(),
        line_through: false,
        decoration_group: 0,
        decoration_role: ColorRole::None,
        table_cell: false,
        text_align: String::new(),
    };
    let root_attrs = measure_markdown_node(
        ruler,
        &legacy_dom,
        legacy_body,
        MeasureArgs {
            font_family,
            mono_font_family,
            font_size,
            font_scale: 1.0,
            font_style: FontStyle::Regular,
            bold: false,
            italic: false,
            css: false,
        },
    );
    let content_width = root_attrs.width.ceil() as i64;
    let content_height = root_attrs.height.ceil() as i64;
    let mut painter = Painter {
        ruler,
        dom: &dom,
        primitives: Vec::new(),
    };
    painter.paint_block(body, 0.0, 0.0, &ctx, content_width as f64);
    let primitives = painter.primitives;
    ruler.line_height_factor = original_line_height;
    ruler.bounds_with_dot = original_bounds_with_dot;
    let corpus = primitives
        .iter()
        .filter(|p| p.kind == PrimitiveKind::Text)
        .map(|p| p.text.as_str())
        .collect();
    Ok(MarkdownLayout {
        width: content_width,
        height: content_height,
        primitives,
        corpus,
    })
}
