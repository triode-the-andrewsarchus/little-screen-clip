// SPDX-License-Identifier: MPL-2.0
// Port of D2 v0.9.0 lib/textmeasure/markdown.go: the github-markdown.css
// constants, HeaderToFontSize/Scale, the html.Node sibling helpers,
// normalizeMarkdownWhitespaceTree and measureMarkdownNode (both the legacy
// graph-layout mode and the cssLineBoxes mode used by native SVG painting).

use super::ruler::FontFamily;
use d2_rs_text::d2fonts::FontStyle;

use super::dom::{Dom, NodeId, NodeType};
use super::ruler::{FontSpec, Ruler};

pub(crate) const MARKDOWN_LINE_HEIGHT: f64 = 1.5;
pub(crate) const PADDING_LEFT_UL_OL_EM: f64 = 2.0;
const MARGIN_BOTTOM_UL: f64 = 16.0;
const MARGIN_TOP_LI_P: f64 = 16.0;
const MARGIN_TOP_LI_EM: f64 = 0.25;
const MARGIN_BOTTOM_P: f64 = 16.0;
pub(crate) const LINE_HEIGHT_H: f64 = 1.25;
const MARGIN_TOP_H: f64 = 24.0;
const MARGIN_BOTTOM_H: f64 = 16.0;
const PADDING_BOTTOM_H1_H2_EM: f64 = 0.3;
pub(crate) const BORDER_BOTTOM_H1_H2: f64 = 1.0;
pub(crate) const HEIGHT_HR_EM: f64 = 0.25;
const MARGIN_TOP_BOTTOM_HR: f64 = 24.0;
pub(crate) const PADDING_PRE: f64 = 16.0;
const MARGIN_BOTTOM_PRE: f64 = 16.0;
const MARGIN_BOTTOM_TABLE: f64 = 16.0;
pub(crate) const LINE_HEIGHT_PRE: f64 = 1.45;
pub(crate) const FONT_SIZE_PRE_CODE_EM: f64 = 0.85;
pub(crate) const PADDING_TOP_BOTTOM_CODE_EM: f64 = 0.2;
pub(crate) const PADDING_LEFT_RIGHT_CODE_EM: f64 = 0.4;
pub(crate) const PADDING_LEFT_RIGHT_HEADING_CODE_EM: f64 = 0.2;
pub(crate) const PADDING_LR_BLOCKQUOTE_EM: f64 = 1.0;
const MARGIN_BOTTOM_BLOCKQUOTE: f64 = 16.0;
pub(crate) const BORDER_LEFT_BLOCKQUOTE_EM: f64 = 0.25;

/// HeaderToFontSize.
fn header_to_font_size(base: i64, header: &str) -> i64 {
    if !is_markdown_heading(header) {
        return 0;
    }
    (header_to_font_scale(header) * base as f64) as i64
}

/// HeaderToFontScale.
pub(crate) fn header_to_font_scale(header: &str) -> f64 {
    match header {
        "h1" => 2.0,
        "h2" => 1.5,
        "h3" => 1.25,
        "h4" => 1.0,
        "h5" => 0.875,
        "h6" => 0.85,
        _ => 1.0,
    }
}

pub(crate) fn is_markdown_heading(element: &str) -> bool {
    matches!(element, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// go2.Max.
pub(crate) fn go_max(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

impl Dom {
    pub(crate) fn has_markdown_heading_ancestor(&self, n: NodeId) -> bool {
        let mut ancestor = self.node(n).parent;
        while let Some(a) = ancestor {
            let node = self.node(a);
            if node.kind == NodeType::Element && is_markdown_heading(&node.data) {
                return true;
            }
            ancestor = node.parent;
        }
        false
    }

    /// hasAncestorElement.
    pub(crate) fn has_ancestor_element(&self, n: NodeId, el: &str) -> bool {
        let mut ancestor = self.node(n).parent;
        while let Some(a) = ancestor {
            if self.is_element(a, el) {
                return true;
            }
            ancestor = self.node(a).parent;
        }
        false
    }

    fn blank(&self, n: NodeId) -> bool {
        self.node(n).data.trim().is_empty()
    }

    /// hasPrev.
    pub(crate) fn has_prev(&self, n: NodeId) -> bool {
        match self.node(n).prev_sibling {
            None => false,
            Some(p) if self.blank(p) => self.has_prev(p),
            Some(_) => true,
        }
    }

    /// hasNext.
    pub(crate) fn has_next(&self, n: NodeId) -> bool {
        match self.node(n).next_sibling {
            None => false,
            Some(p) if self.blank(p) => self.has_next(p),
            Some(_) => true,
        }
    }

    /// getPrev (which, as in Go, continues through getNext).
    pub(crate) fn get_prev(&self, n: Option<NodeId>) -> Option<NodeId> {
        let n = n?;
        if self.blank(n) {
            if let Some(next) = self.get_next(self.node(n).prev_sibling) {
                return Some(next);
            }
        }
        Some(n)
    }

    /// getNext.
    pub(crate) fn get_next(&self, n: Option<NodeId>) -> Option<NodeId> {
        let n = n?;
        if self.blank(n) {
            if let Some(next) = self.get_next(self.node(n).next_sibling) {
                return Some(next);
            }
        }
        Some(n)
    }

    fn parent_element_type(&self, n: NodeId) -> &str {
        match self.node(n).parent {
            Some(p) if self.node(p).kind == NodeType::Element => &self.node(p).data,
            _ => "",
        }
    }

    pub(crate) fn element_data(&self, n: NodeId) -> Option<&str> {
        let node = self.node(n);
        (node.kind == NodeType::Element).then_some(node.data.as_str())
    }

    /// markdownNodeText.
    pub(crate) fn markdown_node_text(&self, n: NodeId) -> String {
        fn walk(dom: &Dom, n: NodeId, out: &mut String) {
            let node = dom.node(n);
            if node.kind == NodeType::Text {
                out.push_str(&node.data);
                return;
            }
            if node.kind == NodeType::Element && node.data == "br" {
                out.push('\n');
                return;
            }
            for child in dom.children(n) {
                walk(dom, child, out);
            }
        }
        let mut out = String::new();
        walk(self, n, &mut out);
        out
    }
}

/// isBlockElement.
pub(crate) fn is_block_element(el: &str) -> bool {
    matches!(
        el,
        "blockquote"
            | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "hr"
            | "li"
            | "ol"
            | "p"
            | "pre"
            | "ul"
            | "table"
            | "thead"
            | "tbody"
            | "tr"
            | "td"
            | "th"
    )
}

pub(crate) fn is_block_node(dom: &Dom, n: NodeId) -> bool {
    dom.element_data(n).is_some_and(is_block_element)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Extra {
    #[default]
    None,
    /// The int 0 a text node stores in extraData (non-nil).
    Zero,
    Widths(Vec<f64>),
    Sections(Vec<Vec<f64>>),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockAttrs {
    pub width: f64,
    pub height: f64,
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub extra: Extra,
}

impl BlockAttrs {
    pub(crate) fn height(height: f64) -> Self {
        Self {
            height,
            ..Default::default()
        }
    }
    /// blockAttrs.isNotEmpty.
    pub(crate) fn is_not_empty(&self) -> bool {
        self.width != 0.0
            || self.height != 0.0
            || self.margin_top != 0.0
            || self.margin_bottom != 0.0
            || self.extra != Extra::None
    }
}

fn is_collapsible_markdown_whitespace(r: char) -> bool {
    matches!(r, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// normalizeMarkdownWhitespaceTree.
pub(crate) fn normalize_markdown_whitespace_tree(dom: &mut Dom, root: NodeId) {
    struct State {
        at_line_start: bool,
        pending: Option<NodeId>,
    }
    struct Walker<'a> {
        dom: &'a Dom,
        outputs: std::collections::BTreeMap<NodeId, String>,
    }
    impl Walker<'_> {
        fn flush_pending(&mut self, state: &mut State) {
            if let Some(p) = state.pending {
                if !state.at_line_start {
                    self.outputs.get_mut(&p).expect("pending output").push(' ');
                }
            }
            state.pending = None;
        }
        fn walk_inline(&mut self, n: NodeId, state: &mut State) {
            let node = self.dom.node(n);
            match node.kind {
                NodeType::Text => {
                    self.outputs.entry(n).or_default();
                    let data = node.data.clone();
                    for r in data.chars() {
                        if is_collapsible_markdown_whitespace(r) {
                            if state.pending.is_none() {
                                state.pending = Some(n);
                            }
                            continue;
                        }
                        self.flush_pending(state);
                        self.outputs.get_mut(&n).expect("output").push(r);
                        state.at_line_start = false;
                    }
                }
                NodeType::Element => {
                    if node.data == "br" {
                        state.pending = None;
                        state.at_line_start = true;
                    } else if is_block_element(&node.data) {
                        state.pending = None;
                        self.normalize_block(n);
                        state.at_line_start = true;
                    } else if node.data == "img" {
                    } else {
                        let children: Vec<_> = self.dom.children(n).collect();
                        for child in children {
                            self.walk_inline(child, state);
                        }
                    }
                }
                _ => {}
            }
        }
        fn normalize_block(&mut self, n: NodeId) {
            if self.dom.is_element(n, "pre") {
                return;
            }
            let mut state = State {
                at_line_start: true,
                pending: None,
            };
            let children: Vec<_> = self.dom.children(n).collect();
            for child in children {
                self.walk_inline(child, &mut state);
            }
        }
    }
    let mut walker = Walker {
        dom,
        outputs: Default::default(),
    };
    walker.normalize_block(root);
    let outputs = walker.outputs;
    for (node, output) in outputs {
        dom.nodes[node].data = output;
    }
}

/// markdownPreLineCount.
pub(crate) fn markdown_pre_line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut count = text.matches('\n').count() + 1;
    if text.ends_with('\n') {
        count -= 1;
    }
    count
}

/// mergeColumnWidths.
pub(crate) fn merge_column_widths(mut existing: Vec<f64>, new: &[Vec<f64>]) -> Vec<f64> {
    for row in new {
        for (i, &width) in row.iter().enumerate() {
            if i >= existing.len() {
                existing.push(width);
            } else {
                existing[i] = go_max(existing[i], width);
            }
        }
    }
    existing
}

#[derive(Clone, Copy)]
pub(crate) struct MeasureArgs {
    pub font_family: FontFamily,
    pub mono_font_family: FontFamily,
    pub font_size: i64,
    pub font_scale: f64,
    pub font_style: FontStyle,
    pub bold: bool,
    pub italic: bool,
    pub css: bool,
}

/// measureMarkdownNode. The ruler's LineHeightFactor is restored on return,
/// as Go's deferred resets do.
pub(crate) fn measure_markdown_node(
    ruler: &mut Ruler,
    dom: &Dom,
    n: NodeId,
    args: MeasureArgs,
) -> BlockAttrs {
    let original = ruler.line_height_factor;
    let attrs = measure_inner(ruler, dom, n, args);
    ruler.line_height_factor = original;
    attrs
}

fn measure_inner(ruler: &mut Ruler, dom: &Dom, n: NodeId, mut a: MeasureArgs) -> BlockAttrs {
    let font = FontSpec::new(a.font_family, a.font_size, a.font_style);
    let parent_type = dom.parent_element_type(n).to_owned();
    let node = dom.node(n);
    match node.kind {
        NodeType::Text => {
            if !a.css && node.data.trim_matches(['\n', '\t', '\u{8}']).is_empty() {
                return BlockAttrs::default();
            }
            let mut s: &str = &node.data;
            let is_code = parent_type == "pre" || parent_type == "code";
            let is_heading_code = parent_type == "code" && dom.has_markdown_heading_ancestor(n);
            let mut space_widths = 0.0;
            let replaced;
            if !is_code {
                let mut space_width = ruler.space_width(font) * a.font_scale;
                if a.css {
                    if let Some(advance) = ruler.measure_font_advance(font, " ") {
                        space_width = advance * a.font_scale;
                    }
                }
                if a.css {
                    if s.is_empty() {
                        return BlockAttrs::default();
                    }
                    if let Some(rest) = s.strip_prefix(' ') {
                        s = rest;
                        space_widths += space_width;
                    }
                    if let Some(rest) = s.strip_suffix(' ') {
                        s = rest;
                        space_widths += space_width;
                    }
                } else {
                    replaced = s.replace(['\n', '\t'], " ");
                    s = &replaced;
                    if let Some(rest) = s.strip_prefix(' ') {
                        s = rest;
                        if dom.has_prev(n) {
                            space_widths += space_width;
                        }
                    }
                    if let Some(rest) = s.strip_suffix(' ') {
                        s = rest;
                        if dom.has_next(n) {
                            space_widths += space_width;
                        }
                    }
                }
            } else if s.is_empty() {
                return BlockAttrs::default();
            }
            if parent_type == "pre" {
                ruler.line_height_factor = LINE_HEIGHT_PRE;
            }
            let (mut w, mut h) = ruler.measure_precise(font, s);
            if a.css {
                w = match ruler.measure_font_advance(font, s) {
                    Some(advance) => advance,
                    None => ruler.scale_unicode_css(w, font, s),
                };
            } else if !is_code {
                w = ruler.scale_unicode_legacy(w, font, s);
            }
            w *= a.font_scale;
            h *= a.font_scale;
            if is_code && (!is_heading_code || !a.css) {
                w *= FONT_SIZE_PRE_CODE_EM;
                h *= FONT_SIZE_PRE_CODE_EM;
            }
            BlockAttrs {
                width: w + space_widths,
                height: h,
                margin_top: 0.0,
                margin_bottom: 0.0,
                extra: Extra::Zero,
            }
        }
        NodeType::Element => {
            let data = node.data.as_str();
            let mut is_code = false;
            match data {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    if a.css {
                        a.font_scale *= header_to_font_scale(data);
                    } else {
                        a.font_size = header_to_font_size(a.font_size, data);
                    }
                    a.font_style = FontStyle::Semibold;
                    a.bold = false;
                    ruler.line_height_factor = LINE_HEIGHT_H;
                }
                "em" => {
                    a.font_style = FontStyle::Italic;
                    a.italic = true;
                }
                "b" | "strong" => {
                    a.font_style = FontStyle::Bold;
                    a.bold = true;
                }
                "th" => {
                    a.font_style = FontStyle::Semibold;
                    a.bold = true;
                }
                "pre" | "code" => {
                    a.font_family = a.mono_font_family;
                    a.font_style = FontStyle::Regular;
                    is_code = true;
                }
                _ => {}
            }
            let mut block = BlockAttrs::default();
            let css_font_size = a.font_size as f64 * a.font_scale;
            let line_height_px = css_font_size * ruler.line_height_factor;

            if node.first_child.is_some() {
                let first = dom.get_next(node.first_child);
                let last = dom.get_prev(node.last_child);
                let mut blocks: Vec<BlockAttrs> = Vec::new();
                let mut inline_block: Option<BlockAttrs> = None;
                let end_inline_block =
                    |inline_block: &mut Option<BlockAttrs>, blocks: &mut Vec<BlockAttrs>| {
                        let mut b = inline_block.take().expect("inline block");
                        if !is_code && b.height > 0.0 {
                            if a.css {
                                b.height = line_height_px;
                            } else if b.height < line_height_px {
                                b.height = line_height_px;
                            }
                        }
                        blocks.push(b);
                    };
                for child in dom.children(n) {
                    let child_block = measure_markdown_node(ruler, dom, child, a);
                    if is_block_node(dom, child) {
                        if inline_block.is_some() {
                            end_inline_block(&mut inline_block, &mut blocks);
                        }
                        let mut new_block = BlockAttrs {
                            width: child_block.width,
                            height: child_block.height,
                            ..Default::default()
                        };
                        new_block.margin_top = if Some(child) == first && data == "blockquote" {
                            0.0
                        } else {
                            child_block.margin_top
                        };
                        new_block.margin_bottom = if Some(child) == last && data == "blockquote" {
                            0.0
                        } else {
                            child_block.margin_bottom
                        };
                        blocks.push(new_block);
                    } else if dom.is_element(child, "br") {
                        if inline_block.is_some() {
                            end_inline_block(&mut inline_block, &mut blocks);
                        } else {
                            block.height += line_height_px;
                        }
                    } else if child_block.is_not_empty() {
                        match inline_block.as_mut() {
                            None => inline_block = Some(child_block),
                            Some(ib) => {
                                ib.width += child_block.width;
                                ib.height = go_max(ib.height, child_block.height);
                                ib.margin_top = go_max(ib.margin_top, child_block.margin_top);
                                ib.margin_bottom =
                                    go_max(ib.margin_bottom, child_block.margin_bottom);
                            }
                        }
                    }
                }
                if inline_block.is_some() {
                    end_inline_block(&mut inline_block, &mut blocks);
                }
                let mut prev_margin_bottom = 0.0;
                let count = blocks.len();
                for (i, b) in blocks.iter().enumerate() {
                    if i == 0 {
                        block.margin_top = go_max(block.margin_top, b.margin_top);
                    } else {
                        let margin_diff = b.margin_top - prev_margin_bottom;
                        if margin_diff > 0.0 {
                            block.height += margin_diff;
                        }
                    }
                    if i == count - 1 {
                        block.margin_bottom = go_max(block.margin_bottom, b.margin_bottom);
                    } else {
                        block.height += b.margin_bottom;
                        prev_margin_bottom = b.margin_bottom;
                    }
                    block.height += b.height;
                    block.width = go_max(block.width, b.width);
                }
            }

            match data {
                "blockquote" => {
                    let mut border_width = BORDER_LEFT_BLOCKQUOTE_EM * css_font_size;
                    if a.css {
                        border_width = go_max(1.0, border_width.floor());
                    }
                    block.width += 2.0 * PADDING_LR_BLOCKQUOTE_EM * css_font_size + border_width;
                    block.margin_bottom = go_max(block.margin_bottom, MARGIN_BOTTOM_BLOCKQUOTE);
                }
                "p" => {
                    if parent_type == "li" {
                        block.margin_top = go_max(block.margin_top, MARGIN_TOP_LI_P);
                    }
                    block.margin_bottom = go_max(block.margin_bottom, MARGIN_BOTTOM_P);
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    block.margin_top = go_max(block.margin_top, MARGIN_TOP_H);
                    block.margin_bottom = go_max(block.margin_bottom, MARGIN_BOTTOM_H);
                    if data == "h1" || data == "h2" {
                        block.height +=
                            PADDING_BOTTOM_H1_H2_EM * css_font_size + BORDER_BOTTOM_H1_H2;
                    }
                }
                "li" => {
                    block.width += PADDING_LEFT_UL_OL_EM * css_font_size;
                    if a.css && block.height == 0.0 {
                        block.height = line_height_px;
                    }
                    if dom.has_prev(n) {
                        block.margin_top =
                            go_max(block.margin_top, MARGIN_TOP_LI_EM * css_font_size);
                    }
                }
                "ol" | "ul" => {
                    if dom.has_ancestor_element(n, "ul") || dom.has_ancestor_element(n, "ol") {
                        block.margin_top = 0.0;
                        block.margin_bottom = 0.0;
                    } else {
                        block.margin_bottom = go_max(block.margin_bottom, MARGIN_BOTTOM_UL);
                    }
                }
                "pre" => {
                    let line_count = markdown_pre_line_count(&dom.markdown_node_text(n));
                    if a.css && line_count > 0 {
                        let css_content_height = line_count as f64
                            * LINE_HEIGHT_PRE
                            * FONT_SIZE_PRE_CODE_EM
                            * css_font_size;
                        block.height = go_max(block.height, css_content_height);
                    }
                    block.width += 2.0 * PADDING_PRE;
                    block.height += 2.0 * PADDING_PRE;
                    block.margin_bottom = go_max(block.margin_bottom, MARGIN_BOTTOM_PRE);
                }
                "code" => {
                    if parent_type != "pre" {
                        if a.css && dom.has_markdown_heading_ancestor(n) {
                            block.width += 2.0 * PADDING_LEFT_RIGHT_HEADING_CODE_EM * css_font_size;
                            block.height = line_height_px;
                        } else if a.css {
                            let code_font_size = FONT_SIZE_PRE_CODE_EM * css_font_size;
                            block.width += 2.0 * PADDING_LEFT_RIGHT_CODE_EM * code_font_size;
                            if let Some((ascent, descent)) = ruler.css_font_box_metrics(
                                FontSpec::new(a.font_family, a.font_size, a.font_style),
                                code_font_size,
                            ) {
                                block.height = ascent + descent;
                            }
                            block.height += 2.0 * PADDING_TOP_BOTTOM_CODE_EM * code_font_size;
                        } else {
                            block.width += 2.0 * PADDING_LEFT_RIGHT_CODE_EM * css_font_size;
                            block.height += 2.0 * PADDING_TOP_BOTTOM_CODE_EM * css_font_size;
                        }
                    }
                }
                "hr" => {
                    block.height += HEIGHT_HR_EM * css_font_size;
                    block.margin_top = go_max(block.margin_top, MARGIN_TOP_BOTTOM_HR);
                    block.margin_bottom = go_max(block.margin_bottom, MARGIN_TOP_BOTTOM_HR);
                }
                "table" => {
                    let mut column_widths: Vec<f64> = Vec::new();
                    let mut table_height = 0.0;
                    let table_border = 1.0;
                    for child in dom.children(n) {
                        match dom.element_data(child) {
                            Some("tbody" | "thead" | "tfoot") => {
                                let child_attrs = measure_markdown_node(ruler, dom, child, a);
                                table_height += child_attrs.height;
                                if let Extra::Sections(sections) = &child_attrs.extra {
                                    column_widths = merge_column_widths(column_widths, sections);
                                }
                            }
                            Some("tr") => {
                                let row_attrs = measure_markdown_node(ruler, dom, child, a);
                                table_height += row_attrs.height;
                                if let Extra::Widths(widths) = row_attrs.extra {
                                    column_widths = merge_column_widths(column_widths, &[widths]);
                                }
                            }
                            _ => {}
                        }
                    }
                    let mut table_width = 0.0;
                    if !column_widths.is_empty() {
                        for w in &column_widths {
                            table_width += w;
                        }
                        table_width += (column_widths.len() + 1) as f64 * table_border;
                    }
                    table_height += 2.0 * table_border;
                    block.width = table_width;
                    block.height = table_height;
                    if a.css {
                        block.margin_bottom = go_max(block.margin_bottom, MARGIN_BOTTOM_TABLE);
                    }
                }
                "thead" | "tbody" | "tfoot" => {
                    let (mut section_width, mut section_height) = (0.0, 0.0);
                    let mut sections = Vec::new();
                    for child in dom.children(n) {
                        if dom.is_element(child, "tr") {
                            let child_attrs = measure_markdown_node(ruler, dom, child, a);
                            section_height += child_attrs.height;
                            section_width = go_max(section_width, child_attrs.width);
                            if let Extra::Widths(widths) = child_attrs.extra {
                                sections.push(widths);
                            }
                        }
                    }
                    block.width = section_width;
                    block.height = section_height;
                    block.extra = Extra::Sections(sections);
                }
                "td" | "th" => {
                    if !a.css {
                        let (mut w, mut h) = (0.0, 0.0);
                        for child in dom.children(n) {
                            let child_attrs = measure_markdown_node(
                                ruler,
                                dom,
                                child,
                                MeasureArgs { css: false, ..a },
                            );
                            w = go_max(w, child_attrs.width);
                            h += child_attrs.height;
                        }
                        block.width = w;
                        block.height = h;
                    }
                }
                "tr" => {
                    let mut cell_widths = Vec::new();
                    let mut max_cell_height: f64 = 0.0;
                    let mut cell_count = 0;
                    let in_header = dom.has_ancestor_element(n, "thead");
                    let (mut row_style, mut row_bold) = (a.font_style, a.bold);
                    if in_header {
                        row_style = FontStyle::Semibold;
                        row_bold = true;
                    }
                    for child in dom.children(n) {
                        if let Some(cell @ ("td" | "th")) = dom.element_data(child) {
                            cell_count += 1;
                            let (mut style, mut bold) = (row_style, row_bold);
                            if cell == "th" {
                                style = FontStyle::Semibold;
                                bold = true;
                            }
                            let child_attrs = measure_markdown_node(
                                ruler,
                                dom,
                                child,
                                MeasureArgs {
                                    font_style: style,
                                    bold,
                                    ..a
                                },
                            );
                            cell_widths.push(child_attrs.width + 26.0);
                            max_cell_height = go_max(max_cell_height, child_attrs.height + 12.0);
                        }
                    }
                    let mut row_width = 0.0;
                    if cell_count > 0 {
                        for w in &cell_widths {
                            row_width += w;
                        }
                        row_width += (cell_count + 1) as f64;
                    }
                    block.width = row_width;
                    block.height = max_cell_height + 1.0;
                    block.extra = Extra::Widths(cell_widths);
                }
                _ => {}
            }
            if !a.css && block.height > 0.0 && block.height < line_height_px {
                block.height = line_height_px;
            }
            block
        }
        _ => BlockAttrs::default(),
    }
}
