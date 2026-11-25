mod excerpt;
pub use excerpt::{MarkdownExcerptData, MarkdownExcerptView};
mod images;
pub(crate) use images::ImageCache;
mod math;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum PreviewKey {
    Block(u64),
    Span { block: u64, span: usize },
}
pub(super) type Previews = std::collections::HashMap<PreviewKey, crate::diagram_preview::Preview>;

#[derive(Clone, Debug)]
pub(super) struct LinkTarget {
    pub(super) raw_url: String,
    pub(super) internal_path: Option<std::path::PathBuf>,
    pub(super) dash_color: Color32,
}

impl LinkTarget {
    pub(super) fn is_internal(&self) -> bool {
        self.internal_path.is_some()
    }

    pub(super) fn hover_label(&self) -> String {
        match &self.internal_path {
            Some(path) => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("document");
                format!("Open {name} ({})", self.raw_url)
            }
            None => self.raw_url.clone(),
        }
    }
}

#[derive(Default)]
pub(crate) struct LinkCache {
    base_doc: Option<std::path::PathBuf>,
    resolved: std::collections::HashMap<String, Option<std::path::PathBuf>>,
}

impl LinkCache {
    pub(super) fn set_base_path(&mut self, path: Option<&std::path::Path>) {
        if self.base_doc.as_deref() != path {
            self.base_doc = path.map(std::path::Path::to_path_buf);
            self.resolved.clear();
        }
    }

    pub(super) fn resolve(&mut self, reference: &str) -> Option<std::path::PathBuf> {
        let base_doc = self.base_doc.as_deref()?;
        if let Some(cached) = self.resolved.get(reference) {
            return cached.clone();
        }
        let found = little_clip_core::markdown_assets::resolve_document_link(base_doc, reference);
        if self.resolved.len() >= 512 {
            self.resolved.clear();
        }
        self.resolved.insert(reference.to_owned(), found.clone());
        found
    }
}

#[derive(Default)]
pub(super) struct DocMedia<'a> {
    pub previews: Option<&'a mut Previews>,
    pub images: Option<&'a mut ImageCache>,
    pub links: Option<&'a mut LinkCache>,
    pub navigate_to: Option<&'a mut Option<std::path::PathBuf>>,
}

impl<'a> DocMedia<'a> {
    pub(super) fn reborrow(&mut self) -> DocMedia<'_> {
        DocMedia {
            previews: self.previews.as_deref_mut(),
            images: self.images.as_deref_mut(),
            links: self.links.as_deref_mut(),
            navigate_to: self.navigate_to.as_deref_mut(),
        }
    }

    pub(super) fn resolve_target(&mut self, raw_url: &str, dash_color: Color32) -> LinkTarget {
        let internal_path = self
            .links
            .as_deref_mut()
            .and_then(|cache| cache.resolve(raw_url));
        LinkTarget {
            raw_url: raw_url.to_owned(),
            internal_path,
            dash_color,
        }
    }
}

pub(super) fn is_external_openable_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('#') {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
}

pub(super) fn activate_link_target(
    ui: &egui::Ui,
    target: &LinkTarget,
    navigate_to: Option<&mut Option<std::path::PathBuf>>,
) {
    if let Some(path) = &target.internal_path {
        if let Some(slot) = navigate_to {
            *slot = Some(path.clone());
        }
        ui.ctx().request_repaint();
    } else if is_external_openable_url(&target.raw_url) {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&target.raw_url));
    }
}

pub(super) fn paint_dashed_h_line(
    painter: &egui::Painter,
    x0: f32,
    x1: f32,
    y: f32,
    stroke: Stroke,
    dash_len: f32,
    gap_len: f32,
) {
    if x1 <= x0 {
        return;
    }
    let mut x = x0;
    while x < x1 {
        let end = (x + dash_len).min(x1);
        painter.line_segment([egui::pos2(x, y), egui::pos2(end, y)], stroke);
        x += dash_len + gap_len;
    }
}

use super::*;
use little_clip_core::palette::{contrast_ratio, relative_luminance};
use little_clip_render::markdown::{
    ImagePresentation, InlineCss, ListItem, MarkdownDocBlock, MarkdownLine, MarkdownSpan,
    MarkdownTable, QuoteKind, RgbaColor, TableAlignment, TextAlign,
};

pub(super) type MarkdownBlock = MarkdownDocBlock;

const MAX_READING_WIDTH: f32 = 760.0;
const TABLE_CELL_PAD_X: f32 = 10.0;
const TABLE_CELL_PAD_Y: f32 = 6.0;
const TABLE_MIN_COL_WIDTH: f32 = 84.0;
const TABLE_SCROLL_MAX_COL_WIDTH: f32 = 240.0;

#[derive(Clone, Copy)]
struct BlockStyleContext {
    list_depth: usize,
    compact_spacing: bool,
    align: egui::Align,
    text_color: Color32,
    bg_color: Color32,
    prose_size: f32,
    base_italics: bool,
}

impl BlockStyleContext {
    fn root(palette: &AppPalette) -> Self {
        Self {
            list_depth: 0,
            compact_spacing: false,
            align: egui::Align::LEFT,
            text_color: hex_to_c32(&palette.colors.text_secondary),
            bg_color: hex_to_c32(&palette.colors.canvas),
            prose_size: BODY_FONT_SIZE,
            base_italics: false,
        }
    }
}

fn map_text_align(align: TextAlign) -> egui::Align {
    match align {
        TextAlign::Left => egui::Align::LEFT,
        TextAlign::Center => egui::Align::Center,
        TextAlign::Right => egui::Align::RIGHT,
    }
}

fn blend_rgba_on(bg: Color32, raw: RgbaColor) -> Color32 {
    let a = f32::from(raw.a) / 255.0;
    let inv = 1.0 - a;
    Color32::from_rgb(
        (f32::from(raw.r) * a + f32::from(bg.r()) * inv).round() as u8,
        (f32::from(raw.g) * a + f32::from(bg.g()) * inv).round() as u8,
        (f32::from(raw.b) * a + f32::from(bg.b()) * inv).round() as u8,
    )
}

pub(super) fn css_text_color_on(bg: Color32, raw: RgbaColor, palette: &AppPalette) -> Color32 {
    let fg = blend_rgba_on(bg, raw);
    let bg_rgb = (bg.r(), bg.g(), bg.b());
    let fg_rgb = (fg.r(), fg.g(), fg.b());
    if contrast_ratio(bg_rgb, fg_rgb) >= 4.5 {
        return fg;
    }
    let dark_bg = relative_luminance(bg_rgb.0, bg_rgb.1, bg_rgb.2) < 0.35;
    let target = if dark_bg {
        (255u8, 255u8, 255u8)
    } else {
        (0u8, 0u8, 0u8)
    };
    let mut lo = 0.0_f32;
    let mut hi = 1.0_f32;
    for _ in 0..8 {
        let mid = (lo + hi) * 0.5;
        let cand = (
            (f32::from(fg_rgb.0) + (f32::from(target.0) - f32::from(fg_rgb.0)) * mid).round() as u8,
            (f32::from(fg_rgb.1) + (f32::from(target.1) - f32::from(fg_rgb.1)) * mid).round() as u8,
            (f32::from(fg_rgb.2) + (f32::from(target.2) - f32::from(fg_rgb.2)) * mid).round() as u8,
        );
        if contrast_ratio(bg_rgb, cand) >= 4.5 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let adjusted = (
        (f32::from(fg_rgb.0) + (f32::from(target.0) - f32::from(fg_rgb.0)) * hi).round() as u8,
        (f32::from(fg_rgb.1) + (f32::from(target.1) - f32::from(fg_rgb.1)) * hi).round() as u8,
        (f32::from(fg_rgb.2) + (f32::from(target.2) - f32::from(fg_rgb.2)) * hi).round() as u8,
    );
    if contrast_ratio(bg_rgb, adjusted) >= 4.5 {
        Color32::from_rgb(adjusted.0, adjusted.1, adjusted.2)
    } else {
        crate::ui_ext::contrast_text_c32_on(bg, bg, Some(fg), palette)
    }
}

fn inline_css_bg(css: InlineCss, bg_color: Color32, palette: &AppPalette) -> (Color32, bool) {
    if let Some(raw_bg) = css.background.filter(|c| c.a > 0) {
        (blend_rgba_on(bg_color, raw_bg), true)
    } else if css.mark {
        let highlight = RgbaColor {
            r: 235,
            g: 190,
            b: 55,
            a: 85,
        };
        (blend_rgba_on(bg_color, highlight), true)
    } else if css.kbd || css.monospace {
        (hex_to_c32(&palette.colors.surface_panel), true)
    } else {
        (bg_color, false)
    }
}

fn inline_css_fg(
    css: InlineCss,
    has_link: bool,
    effective_bg: Color32,
    prose_color: Color32,
    bold_color: Color32,
    palette: &AppPalette,
) -> Color32 {
    if let Some(raw_fg) = css.color {
        css_text_color_on(effective_bg, raw_fg, palette)
    } else if has_link {
        crate::ui_ext::contrast_text_c32_on(
            effective_bg,
            effective_bg,
            Some(link_color(palette)),
            palette,
        )
    } else if css.strikethrough {
        hex_to_c32(&palette.colors.text_muted)
    } else if css.kbd || css.monospace {
        inline_code_fg(palette)
    } else if css.mark {
        crate::ui_ext::contrast_text_c32_on(effective_bg, effective_bg, None, palette)
    } else if css.muted {
        crate::ui_ext::contrast_text_c32_on(
            effective_bg,
            effective_bg,
            Some(hex_to_c32(&palette.colors.text_muted)),
            palette,
        )
    } else if css.bold {
        bold_color
    } else {
        prose_color
    }
}

pub(super) fn build_inline_css_text_format(
    css: InlineCss,
    has_link: bool,
    base_size: f32,
    prose_color: Color32,
    bold_color: Color32,
    bg_color: Color32,
    palette: &AppPalette,
) -> egui::TextFormat {
    let (effective_bg, has_bg) = inline_css_bg(css, bg_color, palette);
    let color = inline_css_fg(
        css,
        has_link,
        effective_bg,
        prose_color,
        bold_color,
        palette,
    );
    let mono = css.monospace || css.kbd;
    let size = if css.superscript || css.subscript {
        (base_size * 0.78).max(11.0)
    } else if css.small {
        (base_size * 0.80).max(12.0)
    } else if mono {
        (base_size * 0.9).max(13.0)
    } else {
        base_size
    };
    let font_id = if mono {
        FontId::monospace(size)
    } else {
        FontId::proportional(size)
    };
    let valign = if css.superscript {
        egui::Align::TOP
    } else if css.subscript {
        egui::Align::BOTTOM
    } else {
        egui::Align::Center
    };

    egui::TextFormat {
        font_id,
        color,
        background: if has_bg {
            effective_bg
        } else {
            Color32::TRANSPARENT
        },
        italics: css.italic,
        underline: if css.underline || has_link {
            Stroke::new(1.0_f32, color)
        } else {
            Stroke::NONE
        },
        strikethrough: if css.strikethrough {
            Stroke::new(1.0_f32, color)
        } else {
            Stroke::NONE
        },
        valign,
        ..Default::default()
    }
}

pub(super) fn compute_image_display_size(
    pixel_size: [u32; 2],
    style: ImagePresentation,
    max_width: f32,
    max_height: f32,
) -> Vec2 {
    let nat_w = (pixel_size[0].max(1)) as f32;
    let nat_h = (pixel_size[1].max(1)) as f32;
    let aspect = (nat_w / nat_h).clamp(0.05, 20.0);
    let mut target_w = nat_w;
    let mut target_h = nat_h;
    if let Some(zoom) = style.zoom {
        target_w *= zoom;
        target_h *= zoom;
    }
    if let Some(frac) = style.width_fraction {
        target_w = max_width * frac;
        target_h = target_w / aspect;
    } else if let (Some(w), Some(h)) = (style.width_px, style.height_px) {
        target_w = w;
        target_h = h;
    } else if let Some(w) = style.width_px {
        target_w = w;
        target_h = w / aspect;
    } else if let Some(h) = style.height_px {
        target_h = h;
        target_w = h * aspect;
    }

    if target_w > max_width {
        let scale = (max_width / target_w).max(0.01);
        target_w *= scale;
        target_h *= scale;
    }
    if target_h > max_height {
        let scale = (max_height / target_h).max(0.01);
        target_w *= scale;
        target_h *= scale;
    }
    Vec2::new(target_w.max(12.0), target_h.max(12.0))
}

pub(super) fn image_rounding(style: ImagePresentation, size: Vec2) -> egui::CornerRadius {
    let r = style.border_radius.unwrap_or(4.0);
    egui::CornerRadius::from(r.clamp(0.0, size.x.min(size.y) * 0.5))
}

pub(super) fn parse_blocks(content: &str) -> Vec<MarkdownBlock> {
    little_clip_render::markdown::parse_blocks(content)
}

pub(super) fn prepare_blocks(lines: impl IntoIterator<Item = MarkdownLine>) -> Vec<MarkdownBlock> {
    little_clip_render::markdown::blocks_from_lines(lines)
}

pub(super) fn child_path(parent: u64, index: usize) -> u64 {
    parent
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add((index as u64).wrapping_add(1))
}

pub(super) fn inline_code_fg(palette: &AppPalette) -> Color32 {
    let code_bg = hex_to_c32(&palette.colors.surface_panel);
    crate::ui_ext::contrast_text_c32_on(
        code_bg,
        code_bg,
        Some(hex_to_c32(&palette.harmonies.triadic_2)),
        palette,
    )
}

pub(super) fn link_color(palette: &AppPalette) -> Color32 {
    let canvas = hex_to_c32(&palette.colors.canvas);
    crate::ui_ext::contrast_text_c32_on(
        canvas,
        canvas,
        Some(hex_to_c32(&palette.url_color)),
        palette,
    )
}

impl TextPreviewState {
    /// Retires read-only diagram and image jobs when the document presentation is hidden.
    pub fn cancel_diagram_previews(&mut self) {
        self.diagram_previews.clear();
        self.md_images.clear();
    }

    /// Renders styled markdown view inside a centered reading column.
    pub(super) fn render_markdown_view(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        for preview in self.diagram_previews.values_mut() {
            preview.tick(ui.ctx());
        }
        self.md_images.set_base_path(self.path.as_deref());
        self.md_links.set_base_path(self.path.as_deref());
        let blocks = self.markdown.get_or_init(|| parse_blocks(&self.content));
        let media = DocMedia {
            previews: Some(&mut self.diagram_previews),
            images: Some(&mut self.md_images),
            links: Some(&mut self.md_links),
            navigate_to: Some(&mut self.pending_navigation),
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_reading_column(ui, blocks, palette, media);
            });
    }
}

pub fn render_lines(
    ui: &mut egui::Ui,
    parsed: impl IntoIterator<Item = MarkdownLine>,
    palette: &AppPalette,
) {
    let blocks = prepare_blocks(parsed);
    render_reading_column(ui, &blocks, palette, DocMedia::default());
}

fn render_reading_column(
    ui: &mut egui::Ui,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    media: DocMedia<'_>,
) {
    let avail_w = ui.available_width();
    let side_pad = if avail_w > MAX_READING_WIDTH + 48.0 {
        (avail_w - MAX_READING_WIDTH) * 0.5
    } else if avail_w >= 480.0 {
        24.0
    } else {
        4.0
    };
    let col_w = (avail_w - side_pad * 2.0).max(1.0);
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(side_pad);
        ui.allocate_ui_with_layout(
            Vec2::new(col_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_max_width(col_w);
                render_blocks_with_media(ui, blocks, palette, media);
            },
        );
    });
    ui.add_space(20.0);
}

#[cfg(test)]
pub(super) fn render_blocks(
    ui: &mut egui::Ui,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    previews: Option<&mut Previews>,
) {
    render_blocks_with_media(
        ui,
        blocks,
        palette,
        DocMedia {
            previews,
            images: None,
            links: None,
            navigate_to: None,
        },
    );
}

pub(super) fn render_blocks_with_media(
    ui: &mut egui::Ui,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    media: DocMedia<'_>,
) {
    render_block_slice(
        ui,
        blocks,
        palette,
        0,
        true,
        BlockStyleContext::root(palette),
        media,
    );
}

struct BlockImageItem<'a> {
    alt: &'a str,
    url: &'a str,
    href: Option<&'a str>,
    style: ImagePresentation,
}

fn extract_pure_images(spans: &[MarkdownSpan]) -> Option<Vec<BlockImageItem<'_>>> {
    let mut items = Vec::new();
    for span in spans {
        match span {
            MarkdownSpan::Image { alt, url } => items.push(BlockImageItem {
                alt,
                url,
                href: None,
                style: ImagePresentation::default(),
            }),
            MarkdownSpan::StyledImage {
                alt,
                url,
                href,
                style,
            } => items.push(BlockImageItem {
                alt,
                url,
                href: href.as_deref(),
                style: *style,
            }),
            MarkdownSpan::HardBreak => {}
            MarkdownSpan::Text(t) if t.trim().is_empty() => {}
            _ => return None,
        }
    }
    (!items.is_empty()).then_some(items)
}

fn render_block_slice(
    ui: &mut egui::Ui,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    parent_path: u64,
    is_doc_root: bool,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    let mut i = 0;
    while i < blocks.len() {
        let block_path = if is_doc_root {
            i as u64
        } else {
            child_path(parent_path, i)
        };
        if let MarkdownBlock::Paragraph { spans } = &blocks[i]
            && let Some(imgs) = extract_pure_images(spans)
            && imgs.len() == 1
            && let Some(float_dir) = imgs[0].style.float
            && i + 1 < blocks.len()
            && matches!(
                &blocks[i + 1],
                MarkdownBlock::Paragraph { .. }
                    | MarkdownBlock::Heading { .. }
                    | MarkdownBlock::List { .. }
            )
            && ui.available_width() >= 380.0
            && render_floated_pair(
                ui,
                &imgs[0],
                &blocks[i + 1],
                float_dir,
                palette,
                block_path,
                child_path(parent_path, i + 1),
                ctx,
                media.reborrow(),
            )
        {
            i += 2;
            continue;
        }

        render_block(ui, &blocks[i], palette, block_path, ctx, media.reborrow());
        i += 1;
    }
}

fn resolve_image_link(
    media: &mut DocMedia<'_>,
    item: &BlockImageItem<'_>,
    dash_color: Color32,
) -> Option<LinkTarget> {
    item.href
        .map(|href| media.resolve_target(href, dash_color))
        .or_else(|| {
            is_web_url(item.url).then(|| LinkTarget {
                raw_url: item.url.to_owned(),
                internal_path: None,
                dash_color,
            })
        })
}

#[allow(clippy::too_many_arguments)]
fn render_floated_pair(
    ui: &mut egui::Ui,
    img: &BlockImageItem<'_>,
    partner: &MarkdownBlock,
    float_dir: TextAlign,
    palette: &AppPalette,
    img_path: u64,
    partner_path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) -> bool {
    let avail_w = ui.available_width().max(1.0);
    let max_img_w = (avail_w * 0.45).max(80.0);
    let img_link = resolve_image_link(&mut media, img, link_color(palette));
    let Some(images) = media.images.as_deref_mut() else {
        return false;
    };
    let crate::text_preview::markdown::images::ImageStatus::Ready {
        texture,
        pixel_size,
    } = images.resolve(ui, img.url)
    else {
        return false;
    };
    let img_size = compute_image_display_size(pixel_size, img.style, max_img_w, 380.0);
    let text_w = (avail_w - img_size.x - 16.0).max(120.0);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        if float_dir == TextAlign::Left {
            paint_ready_image(
                ui,
                &texture,
                img,
                img_link.as_ref(),
                img_size,
                palette,
                img_path,
                media.navigate_to.as_deref_mut(),
            );
            ui.allocate_ui_with_layout(
                Vec2::new(text_w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_max_width(text_w);
                    render_block(ui, partner, palette, partner_path, ctx, media.reborrow());
                },
            );
        } else {
            ui.allocate_ui_with_layout(
                Vec2::new(text_w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_max_width(text_w);
                    render_block(ui, partner, palette, partner_path, ctx, media.reborrow());
                },
            );
            paint_ready_image(
                ui,
                &texture,
                img,
                img_link.as_ref(),
                img_size,
                palette,
                img_path,
                media.navigate_to.as_deref_mut(),
            );
        }
    });
    ui.add_space(6.0);
    true
}

fn render_image_row(
    ui: &mut egui::Ui,
    imgs: &[BlockImageItem<'_>],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    let avail_w = ui.available_width().max(1.0);
    let effective_align = imgs[0]
        .style
        .align
        .or(imgs[0].style.float)
        .map(map_text_align)
        .unwrap_or(ctx.align);
    let per_item_max_w = if imgs.len() > 1 {
        ((avail_w - 8.0 * (imgs.len() - 1) as f32) / imgs.len() as f32).max(48.0)
    } else {
        avail_w
    };

    let layout = match effective_align {
        egui::Align::Center => egui::Layout::left_to_right(egui::Align::Center)
            .with_main_align(egui::Align::Center)
            .with_main_wrap(true),
        egui::Align::RIGHT => egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
        _ => egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
    };
    ui.allocate_ui_with_layout(Vec2::new(avail_w, 0.0), layout, |ui| {
        for (idx, item) in imgs.iter().enumerate() {
            let item_path = child_path(path, idx);
            let link_target = resolve_image_link(&mut media, item, link_color(palette));
            if let Some(images) = media.images.as_deref_mut() {
                match images.resolve(ui, item.url) {
                    images::ImageStatus::Ready {
                        texture,
                        pixel_size,
                    } => {
                        let size = compute_image_display_size(
                            pixel_size,
                            item.style,
                            per_item_max_w,
                            540.0,
                        );
                        paint_ready_image(
                            ui,
                            &texture,
                            item,
                            link_target.as_ref(),
                            size,
                            palette,
                            item_path,
                            media.navigate_to.as_deref_mut(),
                        );
                        continue;
                    }
                    images::ImageStatus::Loading => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let label = if item.alt.is_empty() {
                                "Loading image…".to_owned()
                            } else {
                                item.alt.to_owned()
                            };
                            ui.small(label);
                        });
                        continue;
                    }
                    images::ImageStatus::Unavailable => {}
                }
            }
            let badge_label = if item.alt.is_empty() {
                format!("[image: {}]", item.url)
            } else {
                format!("[image: {}]", item.alt)
            };
            render_image_fallback_badge(
                ui,
                &badge_label,
                link_target.as_ref(),
                palette,
                media.navigate_to.as_deref_mut(),
            );
        }
    });
}

fn is_web_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

#[allow(clippy::too_many_arguments)]
fn paint_ready_image(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    item: &BlockImageItem<'_>,
    link_target: Option<&LinkTarget>,
    size: Vec2,
    palette: &AppPalette,
    path: u64,
    navigate_to: Option<&mut Option<std::path::PathBuf>>,
) {
    let sense = if link_target.is_some() {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let _ = path;
    let rounding = image_rounding(item.style, size);
    egui::Image::new(texture)
        .corner_radius(rounding)
        .paint_at(ui, rect);
    ui.painter().rect_stroke(
        rect,
        rounding,
        Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.chrome_border, 90)),
        egui::StrokeKind::Inside,
    );
    if let Some(target) = link_target {
        if target.is_internal() {
            let stroke_w = if response.hovered() {
                1.75_f32
            } else {
                1.25_f32
            };
            paint_dashed_h_line(
                ui.painter(),
                rect.left(),
                rect.right(),
                rect.bottom() + 2.0,
                Stroke::new(stroke_w, target.dash_color),
                5.0,
                3.0,
            );
        }
        if response.clicked() {
            activate_link_target(ui, target, navigate_to);
        }
        let target_hover = target.hover_label();
        let hover = if item.alt.is_empty() {
            target_hover
        } else {
            format!("{}\n{target_hover}", item.alt)
        };
        response
            .on_hover_text(hover)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    } else if !item.alt.is_empty() {
        response.on_hover_text(item.alt);
    }
}

fn render_image_fallback_badge(
    ui: &mut egui::Ui,
    label: &str,
    link_target: Option<&LinkTarget>,
    palette: &AppPalette,
    navigate_to: Option<&mut Option<std::path::PathBuf>>,
) {
    let color = if link_target.is_some() {
        link_color(palette)
    } else {
        hex_to_c32(&palette.colors.text_muted)
    };
    let mut rich = egui::RichText::new(label)
        .italics()
        .size(BODY_FONT_SIZE)
        .color(color);
    if link_target.is_some_and(|t| !t.is_internal()) {
        rich = rich.underline();
    }
    let response = ui.add(egui::Label::new(rich).sense(if link_target.is_some() {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    }));
    if let Some(target) = link_target {
        if target.is_internal() {
            let stroke_w = if response.hovered() {
                1.5_f32
            } else {
                1.25_f32
            };
            paint_dashed_h_line(
                ui.painter(),
                response.rect.left(),
                response.rect.right(),
                response.rect.bottom() - 1.0,
                Stroke::new(stroke_w, target.dash_color),
                4.0,
                2.5,
            );
        }
        if response.clicked() {
            activate_link_target(ui, target, navigate_to);
        }
        response
            .on_hover_text(target.hover_label())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

fn render_frontmatter_block(
    ui: &mut egui::Ui,
    metadata: &little_clip_core::markdown_metadata::FrontMatter,
    prose_color: Color32,
    bold_color: Color32,
    path: u64,
) {
    egui::CollapsingHeader::new(format!("{} metadata", metadata.format.name()))
        .id_salt(("frontmatter", path))
        .default_open(true)
        .show(ui, |ui| {
            if let Err(error) = &metadata.value {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
            egui::Grid::new(("frontmatter_grid", path))
                .striped(true)
                .spacing(Vec2::new(16.0, 6.0))
                .show(ui, |ui| {
                    for (key, value) in metadata.display_fields() {
                        ui.label(
                            egui::RichText::new(key)
                                .size(BODY_FONT_SIZE)
                                .color(bold_color),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(value)
                                    .size(BODY_FONT_SIZE)
                                    .color(prose_color),
                            )
                            .wrap(),
                        );
                        ui.end_row();
                    }
                });
        });
    ui.add_space(crate::ui_ext::SPACE_SM);
}

fn quote_accent_and_label(
    kind: Option<QuoteKind>,
    palette: &AppPalette,
) -> (Color32, Option<&'static str>) {
    match kind {
        Some(QuoteKind::Note) => (hex_to_c32(&palette.harmonies.base), Some("Note")),
        Some(QuoteKind::Tip) => (hex_to_c32(&palette.diff_add), Some("Tip")),
        Some(QuoteKind::Important) => (hex_to_c32(&palette.harmonies.triadic_1), Some("Important")),
        Some(QuoteKind::Warning) => (
            hex_to_c32(&palette.harmonies.warm_attention),
            Some("Warning"),
        ),
        Some(QuoteKind::Caution) => (hex_to_c32(&palette.diff_del), Some("Caution")),
        None => (hex_to_c32(&palette.harmonies.base), None),
    }
}

fn render_quote_block(
    ui: &mut egui::Ui,
    kind: Option<QuoteKind>,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    let (accent, label) = quote_accent_and_label(kind, palette);
    let bg = hex_to_c32(&palette.colors.surface_recessed);
    let frame_response = egui::Frame::NONE
        .fill(bg)
        .inner_margin(egui::Margin {
            left: 14,
            right: 12,
            top: 8,
            bottom: 6,
        })
        .corner_radius(egui::CornerRadius::from(crate::ui_ext::BADGE_CORNER_RADIUS))
        .show(ui, |ui| {
            if let Some(title) = label {
                let title_color =
                    crate::ui_ext::contrast_text_c32_on(bg, bg, Some(accent), palette);
                ui.label(
                    egui::RichText::new(title)
                        .size(BODY_FONT_SIZE)
                        .color(title_color),
                );
                ui.add_space(4.0);
            }
            let quote_ctx = BlockStyleContext {
                compact_spacing: true,
                bg_color: bg,
                ..ctx
            };
            render_block_slice(
                ui,
                blocks,
                palette,
                path,
                false,
                quote_ctx,
                media.reborrow(),
            );
        });
    let rect = frame_response.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.left() + 3.0, rect.bottom()),
        ),
        egui::CornerRadius::from(1.5),
        accent,
    );
    ui.add_space(if ctx.compact_spacing { 4.0 } else { 8.0 });
}

fn effective_bold_color(palette: &AppPalette, ctx: BlockStyleContext) -> Color32 {
    if ctx.text_color != hex_to_c32(&palette.colors.text_secondary) {
        ctx.text_color
    } else {
        hex_to_c32(&palette.colors.text_primary)
    }
}

struct DetailsContent<'a> {
    summary: &'a [MarkdownSpan],
    open: bool,
    blocks: &'a [MarkdownBlock],
}

fn render_details_block(
    ui: &mut egui::Ui,
    details: DetailsContent<'_>,
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    let summary_color = effective_bold_color(palette, ctx);
    let built_summary = build_prose_layout_job_with_options(
        details.summary,
        BODY_FONT_SIZE,
        summary_color,
        summary_color,
        ctx.bg_color,
        palette,
        ui.available_width().max(1.0),
        egui::Align::LEFT,
        ProseFormatOptions::default(),
        None,
    );
    egui::Frame::NONE
        .fill(hex_to_c32(&palette.colors.surface_recessed))
        .stroke(Stroke::new(
            1.0_f32,
            hex_to_c32_alpha(&palette.chrome_border, 120),
        ))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .corner_radius(egui::CornerRadius::from(crate::ui_ext::BADGE_CORNER_RADIUS))
        .show(ui, |ui| {
            egui::CollapsingHeader::new(built_summary.job)
                .id_salt(("md_details", path))
                .default_open(details.open)
                .show(ui, |ui| {
                    let details_ctx = BlockStyleContext {
                        compact_spacing: true,
                        bg_color: hex_to_c32(&palette.colors.surface_recessed),
                        ..ctx
                    };
                    render_block_slice(
                        ui,
                        details.blocks,
                        palette,
                        path,
                        false,
                        details_ctx,
                        media.reborrow(),
                    );
                });
        });
    ui.add_space(if ctx.compact_spacing { 4.0 } else { 8.0 });
}

fn render_styled_container_block(
    ui: &mut egui::Ui,
    style: little_clip_render::markdown::BlockBoxStyle,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    if style.margin_top > 0.0 && !ctx.compact_spacing {
        ui.add_space(style.margin_top);
    }
    let has_visible_bg = style.background.is_some_and(|c| c.a > 0);
    let bg = if let Some(raw_bg) = style.background.filter(|c| c.a > 0) {
        blend_rgba_on(ctx.bg_color, raw_bg)
    } else {
        ctx.bg_color
    };
    let text_color = if let Some(raw_fg) = style.text_color {
        css_text_color_on(bg, raw_fg, palette)
    } else if has_visible_bg {
        crate::ui_ext::contrast_text_c32_on(bg, bg, None, palette)
    } else {
        ctx.text_color
    };
    let child_ctx = BlockStyleContext {
        align: style.align.map(map_text_align).unwrap_or(ctx.align),
        text_color,
        bg_color: bg,
        ..ctx
    };
    let has_chrome =
        has_visible_bg || style.border_width > 0.0 || style.padding.iter().any(|p| *p > 0.0);
    let mut draw_children = |ui: &mut egui::Ui| {
        render_block_slice(
            ui,
            blocks,
            palette,
            path,
            false,
            child_ctx,
            media.reborrow(),
        );
    };
    if has_chrome {
        let mut frame = egui::Frame::NONE
            .inner_margin(egui::Margin {
                top: style.padding[0].round().clamp(0.0, 127.0) as i8,
                right: style.padding[1].round().clamp(0.0, 127.0) as i8,
                bottom: style.padding[2].round().clamp(0.0, 127.0) as i8,
                left: style.padding[3].round().clamp(0.0, 127.0) as i8,
            })
            .corner_radius(egui::CornerRadius::from(style.border_radius));
        if has_visible_bg {
            frame = frame.fill(bg);
        }
        if style.border_width > 0.0 {
            let stroke_color = style
                .border_color
                .map(|raw| blend_rgba_on(bg, raw))
                .unwrap_or_else(|| hex_to_c32_alpha(&palette.chrome_border, 160));
            frame = frame.stroke(Stroke::new(style.border_width, stroke_color));
        }
        frame.show(ui, draw_children);
    } else {
        draw_children(ui);
    }
    let trailing = style
        .margin_bottom
        .max(if ctx.compact_spacing { 3.0 } else { 8.0 });
    ui.add_space(trailing);
}

fn render_code_or_diagram_block(
    ui: &mut egui::Ui,
    language: &str,
    text: &str,
    palette: &AppPalette,
    path: u64,
    compact_spacing: bool,
    mut media: DocMedia<'_>,
) {
    if let Some(format) = little_clip_core::diagram::Format::from_language(language)
        && let Some(previews) = media.previews.as_deref_mut()
    {
        previews
            .entry(PreviewKey::Block(path))
            .or_default()
            .show(ui, format, text);
        egui::CollapsingHeader::new("Diagram source")
            .id_salt(("diagram_source", path))
            .show(ui, |ui| {
                render_styled_code_block(ui, text, language, palette, path);
            });
    } else {
        render_styled_code_block(ui, text, language, palette, path);
    }
    ui.add_space(if compact_spacing { 4.0 } else { 8.0 });
}

fn render_footnote_block(
    ui: &mut egui::Ui,
    label: &str,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    egui::Frame::NONE
        .fill(hex_to_c32(&palette.colors.surface_recessed))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .corner_radius(egui::CornerRadius::from(crate::ui_ext::BADGE_CORNER_RADIUS))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.label(
                    egui::RichText::new(format!("[^{label}]"))
                        .size(BODY_FONT_SIZE)
                        .color(hex_to_c32(&palette.colors.text_muted)),
                );
                ui.vertical(|ui| {
                    let fn_ctx = BlockStyleContext {
                        compact_spacing: true,
                        bg_color: hex_to_c32(&palette.colors.surface_recessed),
                        ..ctx
                    };
                    render_block_slice(ui, blocks, palette, path, false, fn_ctx, media.reborrow());
                });
            });
        });
    ui.add_space(6.0);
}

fn render_block(
    ui: &mut egui::Ui,
    block: &MarkdownBlock,
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    let prose_color = ctx.text_color;
    let bold_color = effective_bold_color(palette, ctx);
    let compact_spacing = ctx.compact_spacing;

    match block {
        MarkdownBlock::FrontMatter(metadata) => {
            render_frontmatter_block(ui, metadata, prose_color, bold_color, path);
        }
        MarkdownBlock::Heading { level, spans, .. } => {
            render_heading(ui, *level, spans, palette, path, ctx, media);
        }
        MarkdownBlock::Paragraph { spans } => {
            if let Some(imgs) = extract_pure_images(spans) {
                render_image_row(ui, &imgs, palette, path, ctx, media);
            } else {
                render_spans_with_options(
                    ui,
                    spans,
                    ctx.prose_size,
                    prose_color,
                    bold_color,
                    ctx.bg_color,
                    palette,
                    path,
                    media,
                    ctx.align,
                    ProseFormatOptions {
                        base_italics: ctx.base_italics,
                        ..Default::default()
                    },
                );
            }
            ui.add_space(if compact_spacing { 3.0 } else { 8.0 });
        }
        MarkdownBlock::Quote { kind, blocks } => {
            render_quote_block(ui, *kind, blocks, palette, path, ctx, media);
        }
        MarkdownBlock::PullQuote {
            big,
            source,
            blocks,
        } => render_pull_quote(
            ui,
            *big,
            source,
            blocks,
            palette,
            path,
            ctx,
            media.reborrow(),
        ),
        MarkdownBlock::Tabs { tabs } => {
            render_tabs_block(ui, tabs, palette, path, ctx, media.reborrow());
        }
        MarkdownBlock::Columns {
            dividers,
            bottom_rule,
            columns,
        } => render_columns_block(
            ui,
            *dividers,
            *bottom_rule,
            columns,
            palette,
            path,
            ctx,
            media.reborrow(),
        ),
        MarkdownBlock::Details {
            summary,
            open,
            blocks,
        } => {
            render_details_block(
                ui,
                DetailsContent {
                    summary,
                    open: *open,
                    blocks,
                },
                palette,
                path,
                ctx,
                media,
            );
        }
        MarkdownBlock::StyledContainer { style, blocks } => {
            render_styled_container_block(ui, *style, blocks, palette, path, ctx, media);
        }
        MarkdownBlock::List {
            ordered,
            start,
            items,
        } => {
            render_list(ui, *ordered, *start, items, palette, path, ctx, media);
            ui.add_space(if compact_spacing { 3.0 } else { 8.0 });
        }
        MarkdownBlock::Code { language, text } => {
            render_code_or_diagram_block(ui, language, text, palette, path, compact_spacing, media);
        }
        MarkdownBlock::Table(table) => {
            render_markdown_table(ui, table, palette, path, media);
            ui.add_space(if compact_spacing { 4.0 } else { 10.0 });
        }
        MarkdownBlock::Footnote { label, blocks } => {
            render_footnote_block(ui, label, blocks, palette, path, ctx, media);
        }
        MarkdownBlock::Rule => {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
        }
        MarkdownBlock::IncompleteMath { source } => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Unclosed display equation ($$)",
            );
            ui.monospace(source.chars().take(2048).collect::<String>());
            ui.add_space(6.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_list(
    ui: &mut egui::Ui,
    ordered: bool,
    start: u64,
    items: &[ListItem],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    let depth = ctx.list_depth;
    let bullet_symbol = match depth % 3 {
        0 => "•",
        1 => "◦",
        _ => "▪",
    };
    let marker_color = crate::ui_ext::contrast_text_c32_on(
        ui.visuals().panel_fill,
        ui.visuals().panel_fill,
        Some(hex_to_c32(&palette.harmonies.base)),
        palette,
    );
    let marker_font = FontId::proportional(BODY_FONT_SIZE);
    let gutter_w = if ordered {
        let last_num = start.saturating_add(items.len().saturating_sub(1) as u64);
        let widest = format!("{last_num}.");
        let w = ui.fonts_mut(|f| {
            f.layout_no_wrap(widest, marker_font.clone(), marker_color)
                .size()
                .x
        });
        (w + 8.0).max(24.0)
    } else {
        20.0
    };

    for (idx, item) in items.iter().enumerate() {
        let item_path = child_path(path, idx);
        let indent_pad = if depth > 0 { 8.0 } else { 0.0 };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if indent_pad > 0.0 {
                ui.add_space(indent_pad);
            }
            let row_h = ui.fonts_mut(|f| f.row_height(&marker_font));
            let (marker_rect, _) =
                ui.allocate_exact_size(Vec2::new(gutter_w, row_h), egui::Sense::hover());
            match item.task {
                Some(checked) => paint_task_checkbox(ui, marker_rect, checked, palette),
                None if ordered => {
                    let num_label = format!("{}.", start.saturating_add(idx as u64));
                    ui.painter().text(
                        egui::pos2(marker_rect.right() - 6.0, marker_rect.top()),
                        egui::Align2::RIGHT_TOP,
                        num_label,
                        marker_font.clone(),
                        marker_color,
                    );
                }
                None => {
                    ui.painter().text(
                        egui::pos2(marker_rect.center().x, marker_rect.top()),
                        egui::Align2::CENTER_TOP,
                        bullet_symbol,
                        marker_font.clone(),
                        marker_color,
                    );
                }
            }
            let body_w = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                Vec2::new(body_w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_max_width(body_w);
                    let child_ctx = BlockStyleContext {
                        list_depth: depth + 1,
                        compact_spacing: true,
                        ..ctx
                    };
                    render_block_slice(
                        ui,
                        &item.blocks,
                        palette,
                        item_path,
                        false,
                        child_ctx,
                        media.reborrow(),
                    );
                },
            );
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_pull_quote(
    ui: &mut egui::Ui,
    big: bool,
    source: &[MarkdownSpan],
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    let muted = hex_to_c32(&palette.colors.text_muted);
    let compact_spacing = ctx.compact_spacing;
    if big {
        ui.add_space(if compact_spacing { 4.0 } else { 8.0 });
        ui.separator();
        ui.add_space(if compact_spacing { 6.0 } else { 10.0 });
        let avail_w = ui.available_width().max(1.0);
        let inset = (avail_w * 0.08).clamp(8.0, 44.0);
        let inner_w = (avail_w - inset * 2.0).max(1.0);
        ui.horizontal(|ui| {
            ui.add_space(inset);
            ui.allocate_ui_with_layout(
                Vec2::new(inner_w, 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.set_max_width(inner_w);
                    let quote_ctx = BlockStyleContext {
                        compact_spacing: true,
                        align: egui::Align::Center,
                        prose_size: (BODY_FONT_SIZE * 1.2).round(),
                        base_italics: true,
                        ..ctx
                    };
                    render_block_slice(
                        ui,
                        blocks,
                        palette,
                        path,
                        false,
                        quote_ctx,
                        media.reborrow(),
                    );
                    if !source.is_empty() {
                        let mut attr_spans = vec![MarkdownSpan::Text("— ".to_owned())];
                        attr_spans.extend(source.iter().cloned());
                        render_spans_with_options(
                            ui,
                            &attr_spans,
                            (BODY_FONT_SIZE * 0.85).max(13.0),
                            muted,
                            muted,
                            ctx.bg_color,
                            palette,
                            child_path(path, 999),
                            media.reborrow(),
                            egui::Align::Center,
                            ProseFormatOptions::default(),
                        );
                    }
                },
            );
        });
        ui.add_space(if compact_spacing { 6.0 } else { 10.0 });
        ui.separator();
        ui.add_space(if compact_spacing { 4.0 } else { 8.0 });
        return;
    }

    let bg = hex_to_c32(&palette.colors.surface_recessed);
    let stripe = hex_to_c32_alpha(&palette.chrome_border, 220);
    let frame_resp = egui::Frame::NONE
        .fill(bg)
        .inner_margin(egui::Margin {
            left: 16,
            right: 14,
            top: 10,
            bottom: 8,
        })
        .corner_radius(egui::CornerRadius::from(crate::ui_ext::BADGE_CORNER_RADIUS))
        .show(ui, |ui| {
            let quote_ctx = BlockStyleContext {
                compact_spacing: true,
                bg_color: bg,
                ..ctx
            };
            render_block_slice(
                ui,
                blocks,
                palette,
                path,
                false,
                quote_ctx,
                media.reborrow(),
            );
            if !source.is_empty() {
                ui.add_space(2.0);
                let mut attr_spans = vec![MarkdownSpan::Text("— ".to_owned())];
                attr_spans.extend(source.iter().cloned());
                render_spans_with_options(
                    ui,
                    &attr_spans,
                    (BODY_FONT_SIZE * 0.85).max(13.0),
                    muted,
                    muted,
                    bg,
                    palette,
                    child_path(path, 999),
                    media.reborrow(),
                    egui::Align::RIGHT,
                    ProseFormatOptions::default(),
                );
            }
        });
    let rect = frame_resp.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.left() + 3.5, rect.bottom()),
        ),
        egui::CornerRadius::from(1.5),
        stripe,
    );
    ui.add_space(if compact_spacing { 4.0 } else { 8.0 });
}

fn render_tabs_block(
    ui: &mut egui::Ui,
    tabs: &[little_clip_render::markdown::MarkdownTab],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    if tabs.is_empty() {
        return;
    }
    let state_id = ui.make_persistent_id(("md_hugo_tabs", path));
    let mut active = ui
        .data(|d| d.get_temp::<usize>(state_id).unwrap_or(0))
        .min(tabs.len() - 1);

    let panel_bg = hex_to_c32(&palette.colors.surface_recessed);
    let border = hex_to_c32_alpha(&palette.chrome_border, 150);
    egui::Frame::NONE
        .fill(panel_bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(egui::CornerRadius::from(
            crate::ui_ext::CONTROL_CORNER_RADIUS,
        ))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            let items: Vec<clip_ui::ui_tabs::Tab<'_, usize>> = tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| clip_ui::ui_tabs::Tab::new(i, &tab.title))
                .collect();
            if let Some(next) =
                clip_ui::ui_tabs::TabBar::new(("md_tab_bar", path), active, &items, palette)
                    .show(ui)
            {
                active = next.min(tabs.len() - 1);
                ui.data_mut(|d| d.insert_temp(state_id, active));
                ui.ctx().request_repaint();
            }
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            let tab_ctx = BlockStyleContext {
                compact_spacing: true,
                bg_color: panel_bg,
                ..ctx
            };
            render_block_slice(
                ui,
                &tabs[active].blocks,
                palette,
                child_path(path, active),
                false,
                tab_ctx,
                media.reborrow(),
            );
        });
    ui.add_space(if ctx.compact_spacing { 4.0 } else { 10.0 });
}

#[allow(clippy::too_many_arguments)]
fn render_columns_block(
    ui: &mut egui::Ui,
    dividers: bool,
    bottom_rule: bool,
    columns: &[Vec<MarkdownBlock>],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    mut media: DocMedia<'_>,
) {
    if columns.is_empty() {
        return;
    }
    const MIN_COLUMN_WIDTH: f32 = 180.0;
    let avail_w = ui.available_width().max(1.0);
    let gap = if dividers { 18.0 } else { 14.0 };
    let max_per_row = ((avail_w + gap) / (MIN_COLUMN_WIDTH + gap))
        .floor()
        .max(1.0) as usize;
    let divider_color = hex_to_c32_alpha(&palette.chrome_border, 145);

    let mut col_offset = 0usize;
    let chunks: Vec<&[Vec<MarkdownBlock>]> = columns.chunks(max_per_row).collect();
    let chunk_count = chunks.len();
    for (row_idx, row) in chunks.into_iter().enumerate() {
        if row.len() >= 2 {
            let col_w = ((avail_w - gap * (row.len() - 1) as f32) / row.len() as f32).max(1.0);
            let start_x = ui.available_rect_before_wrap().left();
            let row_resp = ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (c_idx, col_blocks) in row.iter().enumerate() {
                    let col_path = child_path(path, col_offset + c_idx);
                    ui.allocate_ui_with_layout(
                        Vec2::new(col_w, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_max_width(col_w);
                            let col_ctx = BlockStyleContext {
                                compact_spacing: true,
                                ..ctx
                            };
                            render_block_slice(
                                ui,
                                col_blocks,
                                palette,
                                col_path,
                                false,
                                col_ctx,
                                media.reborrow(),
                            );
                        },
                    );
                }
            });
            if dividers {
                let rect = row_resp.response.rect;
                let top = rect.top() + 4.0;
                let bottom = rect.bottom() - 4.0;
                if bottom > top {
                    for c_idx in 1..row.len() {
                        let divider_x = start_x + c_idx as f32 * col_w + (c_idx as f32 - 0.5) * gap;
                        ui.painter().vline(
                            divider_x,
                            top..=bottom,
                            Stroke::new(1.0_f32, divider_color),
                        );
                    }
                }
            }
        } else if let Some(col_blocks) = row.first() {
            let col_path = child_path(path, col_offset);
            let col_ctx = BlockStyleContext {
                compact_spacing: true,
                ..ctx
            };
            render_block_slice(
                ui,
                col_blocks,
                palette,
                col_path,
                false,
                col_ctx,
                media.reborrow(),
            );
            if dividers && row_idx + 1 < chunk_count {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
            }
        }
        col_offset += row.len();
        if row_idx + 1 < chunk_count {
            ui.add_space(8.0);
        }
    }

    if bottom_rule {
        ui.add_space(4.0);
        ui.separator();
    }
    ui.add_space(if ctx.compact_spacing { 4.0 } else { 10.0 });
}

fn paint_task_checkbox(ui: &egui::Ui, rect: egui::Rect, checked: bool, palette: &AppPalette) {
    let box_size = 14.0;
    let box_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(box_size));
    let rounding = egui::CornerRadius::from(crate::ui_ext::BADGE_CORNER_RADIUS);
    if checked {
        let fill = hex_to_c32(&palette.diff_add);
        let fg = crate::ui_ext::contrast_text_c32_on(
            fill,
            fill,
            Some(hex_to_c32(&palette.colors.text_primary)),
            palette,
        );
        ui.painter().rect_filled(box_rect, rounding, fill);
        let p0 = egui::pos2(box_rect.left() + 3.0, box_rect.center().y + 0.5);
        let p1 = egui::pos2(box_rect.center().x - 1.0, box_rect.bottom() - 3.5);
        let p2 = egui::pos2(box_rect.right() - 3.0, box_rect.top() + 3.5);
        ui.painter()
            .line_segment([p0, p1], Stroke::new(1.8_f32, fg));
        ui.painter()
            .line_segment([p1, p2], Stroke::new(1.8_f32, fg));
    } else {
        ui.painter().rect_filled(
            box_rect,
            rounding,
            hex_to_c32(&palette.colors.surface_recessed),
        );
        ui.painter().rect_stroke(
            box_rect,
            rounding,
            Stroke::new(1.2_f32, hex_to_c32(&palette.chrome_border)),
            egui::StrokeKind::Inside,
        );
    }
}

struct BuiltProseJob {
    job: egui::text::LayoutJob,
    links: Vec<(std::ops::Range<usize>, LinkTarget)>,
    spoilers: Vec<(std::ops::Range<usize>, usize)>,
}

#[derive(Clone, Copy, Default)]
struct ProseFormatOptions<'a> {
    base_italics: bool,
    extra_letter_spacing: f32,
    line_height: Option<f32>,
    reveal_ctx: Option<(&'a egui::Context, u64)>,
}

struct HeadingStyle {
    size: f32,
    color: Color32,
    top_space: f32,
    bottom_space: f32,
    rule_thickness: f32,
    rule_alpha: u8,
    rule_accent: bool,
    rule_max_width: Option<f32>,
    left_tick: Option<(f32, Color32)>,
    italics: bool,
    extra_letter_spacing: f32,
}

fn heading_style(level: usize, palette: &AppPalette) -> HeadingStyle {
    let primary = hex_to_c32(&palette.colors.text_primary);
    let muted = hex_to_c32(&palette.colors.text_muted);
    let accent = hex_to_c32_alpha(&palette.harmonies.base, 210);
    let neutral_tick = hex_to_c32_alpha(&palette.chrome_border, 190);
    match level {
        1 => HeadingStyle {
            size: 34.0,
            color: primary,
            top_space: 24.0,
            bottom_space: 12.0,
            rule_thickness: 2.0,
            rule_alpha: 210,
            rule_accent: true,
            rule_max_width: None,
            left_tick: None,
            italics: false,
            extra_letter_spacing: -0.3,
        },
        2 => HeadingStyle {
            size: 26.0,
            color: primary,
            top_space: 20.0,
            bottom_space: 9.0,
            rule_thickness: 1.5,
            rule_alpha: 165,
            rule_accent: false,
            rule_max_width: None,
            left_tick: None,
            italics: false,
            extra_letter_spacing: -0.15,
        },
        3 => HeadingStyle {
            size: 22.0,
            color: primary,
            top_space: 16.0,
            bottom_space: 7.0,
            rule_thickness: 0.0,
            rule_alpha: 0,
            rule_accent: false,
            rule_max_width: None,
            left_tick: Some((3.0, accent)),
            italics: false,
            extra_letter_spacing: 0.0,
        },
        4 => HeadingStyle {
            size: 19.0,
            color: primary,
            top_space: 13.0,
            bottom_space: 5.0,
            rule_thickness: 0.0,
            rule_alpha: 0,
            rule_accent: false,
            rule_max_width: None,
            left_tick: Some((2.0, neutral_tick)),
            italics: false,
            extra_letter_spacing: 0.0,
        },
        5 => HeadingStyle {
            size: 17.5,
            color: primary,
            top_space: 10.0,
            bottom_space: 4.0,
            rule_thickness: 1.5,
            rule_alpha: 140,
            rule_accent: false,
            rule_max_width: Some(44.0),
            left_tick: None,
            italics: false,
            extra_letter_spacing: 0.45,
        },
        _ => HeadingStyle {
            size: 16.0,
            color: muted,
            top_space: 8.0,
            bottom_space: 4.0,
            rule_thickness: 0.0,
            rule_alpha: 0,
            rule_accent: false,
            rule_max_width: None,
            left_tick: None,
            italics: true,
            extra_letter_spacing: 0.65,
        },
    }
}

fn render_heading(
    ui: &mut egui::Ui,
    level: usize,
    spans: &[MarkdownSpan],
    palette: &AppPalette,
    path: u64,
    ctx: BlockStyleContext,
    media: DocMedia<'_>,
) {
    let style = heading_style(level, palette);
    let compact_spacing = ctx.compact_spacing;
    let color = if ctx.text_color != hex_to_c32(&palette.colors.text_secondary) {
        ctx.text_color
    } else {
        style.color
    };
    let bold_color = if ctx.text_color != hex_to_c32(&palette.colors.text_secondary) {
        ctx.text_color
    } else {
        hex_to_c32(&palette.colors.text_primary)
    };
    let is_first_doc_block = path == 0 && ctx.list_depth == 0 && !compact_spacing;
    if !is_first_doc_block {
        ui.add_space(if compact_spacing {
            (style.top_space * 0.35).max(3.0)
        } else {
            style.top_space
        });
    }

    let format_opts = ProseFormatOptions {
        base_italics: style.italics,
        extra_letter_spacing: style.extra_letter_spacing,
        line_height: Some((style.size * 1.18).round()),
        reveal_ctx: None,
    };

    if let Some((tick_w, tick_color)) = style.left_tick {
        let frame_response = egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 10,
                right: 0,
                top: 1,
                bottom: 1,
            })
            .show(ui, |ui| {
                render_spans_with_options(
                    ui,
                    spans,
                    style.size,
                    color,
                    bold_color,
                    ctx.bg_color,
                    palette,
                    path,
                    media,
                    ctx.align,
                    format_opts,
                );
            });
        let rect = frame_response.response.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + 2.0),
                egui::pos2(
                    rect.left() + tick_w,
                    (rect.bottom() - 2.0).max(rect.top() + 2.0),
                ),
            ),
            egui::CornerRadius::from(tick_w * 0.5),
            tick_color,
        );
    } else {
        render_spans_with_options(
            ui,
            spans,
            style.size,
            color,
            bold_color,
            ctx.bg_color,
            palette,
            path,
            media,
            ctx.align,
            format_opts,
        );
    }

    if style.rule_thickness > 0.0 {
        ui.add_space(4.0);
        let width = ui.available_width().max(1.0);
        let rule_w = style.rule_max_width.map_or(width, |max_w| width.min(max_w));
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(rule_w, style.rule_thickness + 2.0),
            egui::Sense::hover(),
        );
        let y = rect.center().y;
        let rule_color = hex_to_c32_alpha(&palette.chrome_border, style.rule_alpha);
        ui.painter().line_segment(
            [
                egui::pos2(rect.left(), y),
                egui::pos2(rect.left() + rule_w, y),
            ],
            Stroke::new(style.rule_thickness, rule_color),
        );
        if style.rule_accent {
            let accent_w = (rule_w * 0.25).min(56.0).max(16.0_f32.min(rule_w));
            ui.painter().line_segment(
                [
                    egui::pos2(rect.left(), y),
                    egui::pos2(rect.left() + accent_w, y),
                ],
                Stroke::new(style.rule_thickness, hex_to_c32(&palette.harmonies.base)),
            );
        }
    }

    ui.add_space(if compact_spacing {
        (style.bottom_space * 0.45).max(2.0)
    } else {
        style.bottom_space
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn inline_highlighted_runs(
    code: &str,
    language: &str,
    code_bg: Color32,
    fallback_fg: Color32,
    palette: &AppPalette,
) -> Vec<(String, Color32)> {
    if code.is_empty() {
        return Vec::new();
    }
    if language.trim().is_empty() {
        return vec![(code.to_owned(), fallback_fg)];
    }
    let Some(syn_theme) = THEME_SET.themes.get("base16-eighties.dark") else {
        return vec![(code.to_owned(), fallback_fg)];
    };
    let syntax = find_syntax(language);
    let mut highlighter = HighlightLines::new(syntax, syn_theme);
    let mut runs = Vec::new();
    for line in code.split_inclusive('\n') {
        for (style, token) in highlighter
            .highlight_line(line, &SYNTAX_SET)
            .unwrap_or_default()
        {
            let preferred = map_syntect_color_to_theme(style.foreground, palette);
            let fg =
                crate::ui_ext::contrast_text_c32_on(code_bg, code_bg, Some(preferred), palette);
            runs.push((token.to_owned(), fg));
        }
    }
    if runs.is_empty() {
        runs.push((code.to_owned(), fallback_fg));
    }
    runs
}

pub(super) fn spoiler_is_revealed(ctx: &egui::Context, path: u64, idx: usize) -> bool {
    let open_id = egui::Id::unique(("md_spoiler_open", path, idx));
    let hover_id = egui::Id::unique(("md_spoiler_hover", path, idx));
    ctx.data(|d| {
        d.get_temp::<bool>(open_id).unwrap_or(false)
            || d.get_temp::<bool>(hover_id).unwrap_or(false)
    })
}

pub(super) fn spoiler_mask_color(palette: &AppPalette) -> Color32 {
    let base = hex_to_c32(&palette.colors.surface_panel);
    let mix = hex_to_c32(&palette.chrome_border);
    Color32::from_rgba_unmultiplied(
        ((base.r() as u16 + mix.r() as u16) / 2) as u8,
        ((base.g() as u16 + mix.g() as u16) / 2) as u8,
        ((base.b() as u16 + mix.b() as u16) / 2) as u8,
        255,
    )
}

struct ProseJobBuilder<'a> {
    job: egui::text::LayoutJob,
    links: Vec<(std::ops::Range<usize>, LinkTarget)>,
    spoilers: Vec<(std::ops::Range<usize>, usize)>,
    char_cursor: usize,
    spoiler_counter: usize,
    size: f32,
    prose_color: Color32,
    bold_color: Color32,
    bg_color: Color32,
    code_bg: Color32,
    code_fg: Color32,
    link_fg: Color32,
    muted_fg: Color32,
    palette: &'a AppPalette,
    options: ProseFormatOptions<'a>,
    link_cache: Option<&'a mut LinkCache>,
}

impl<'a> ProseJobBuilder<'a> {
    fn resolve_target(&mut self, url: &str, dash_color: Color32) -> LinkTarget {
        let internal_path = self
            .link_cache
            .as_deref_mut()
            .and_then(|cache| cache.resolve(url));
        LinkTarget {
            raw_url: url.to_owned(),
            internal_path,
            dash_color,
        }
    }

    fn make_format(&self, font_id: FontId, color: Color32, italics: bool) -> egui::TextFormat {
        egui::TextFormat {
            font_id,
            color,
            italics: italics || self.options.base_italics,
            extra_letter_spacing: self.options.extra_letter_spacing,
            line_height: self.options.line_height,
            ..Default::default()
        }
    }

    fn append_text(
        &mut self,
        text: &str,
        mut format: egui::TextFormat,
        link: Option<LinkTarget>,
        spoiler_reveal: Option<bool>,
    ) {
        if text.is_empty() {
            return;
        }
        if let Some(revealed) = spoiler_reveal {
            if revealed {
                if format.background == Color32::TRANSPARENT || format.background == self.bg_color {
                    format.background = self.code_bg;
                }
            } else {
                let mask = spoiler_mask_color(self.palette);
                format.color = mask;
                format.background = mask;
                format.underline = Stroke::NONE;
                format.strikethrough = Stroke::NONE;
            }
        }
        let len_chars = text.chars().count();
        self.job.append(text, 0.0, format);
        if let Some(link) = link
            && spoiler_reveal != Some(false)
        {
            self.links
                .push((self.char_cursor..self.char_cursor + len_chars, link));
        }
        self.char_cursor += len_chars;
    }

    fn append_styled_span(
        &mut self,
        text: &str,
        url: Option<&str>,
        css: InlineCss,
        spoiler_reveal: Option<bool>,
    ) {
        let mut fmt = build_inline_css_text_format(
            css,
            url.is_some(),
            self.size,
            self.prose_color,
            self.bold_color,
            self.bg_color,
            self.palette,
        );
        fmt.italics |= self.options.base_italics;
        if self.options.extra_letter_spacing != 0.0 {
            fmt.extra_letter_spacing = self.options.extra_letter_spacing;
        }
        if self.options.line_height.is_some() {
            fmt.line_height = self.options.line_height;
        }
        let link = url.map(|raw| {
            let target = self.resolve_target(raw, fmt.color);
            if target.is_internal() {
                fmt.underline = Stroke::NONE;
            }
            target
        });
        self.append_text(text, fmt, link, spoiler_reveal);
    }

    fn append_image_placeholder(
        &mut self,
        alt: &str,
        url: &str,
        href: Option<&str>,
        spoiler_reveal: Option<bool>,
    ) {
        let label = if alt.is_empty() {
            "[image]".to_owned()
        } else {
            format!("[image: {alt}]")
        };
        let link_fg = self.link_fg;
        let link = href
            .map(|raw| self.resolve_target(raw, link_fg))
            .or_else(|| {
                (!url.is_empty()).then(|| LinkTarget {
                    raw_url: url.to_owned(),
                    internal_path: None,
                    dash_color: link_fg,
                })
            });
        let mut fmt = self.make_format(FontId::proportional(self.size), self.muted_fg, true);
        if link.as_ref().is_some_and(LinkTarget::is_internal) {
            fmt.underline = Stroke::NONE;
        }
        self.append_text(&label, fmt, link, spoiler_reveal);
    }

    fn append_spans(&mut self, spans: &[MarkdownSpan], spoiler_reveal: Option<bool>) {
        let size = self.size;
        let prose_color = self.prose_color;
        let bold_color = self.bold_color;
        let code_bg = self.code_bg;
        let code_fg = self.code_fg;
        let link_fg = self.link_fg;
        let muted_fg = self.muted_fg;
        for span in spans {
            match span {
                MarkdownSpan::Text(s) => {
                    let fmt = self.make_format(FontId::proportional(size), prose_color, false);
                    self.append_text(s, fmt, None, spoiler_reveal);
                }
                MarkdownSpan::Bold(s) => {
                    let fmt = self.make_format(FontId::proportional(size), bold_color, false);
                    self.append_text(s, fmt, None, spoiler_reveal);
                }
                MarkdownSpan::Italic(s) => {
                    let fmt = self.make_format(FontId::proportional(size), prose_color, true);
                    self.append_text(s, fmt, None, spoiler_reveal);
                }
                MarkdownSpan::BoldItalic(s) => {
                    let fmt = self.make_format(FontId::proportional(size), bold_color, true);
                    self.append_text(s, fmt, None, spoiler_reveal);
                }
                MarkdownSpan::Strikethrough(s) => {
                    let mut fmt = self.make_format(FontId::proportional(size), muted_fg, false);
                    fmt.strikethrough = Stroke::new(1.0_f32, muted_fg);
                    self.append_text(s, fmt, None, spoiler_reveal);
                }
                MarkdownSpan::Code(s) => {
                    let mut fmt =
                        self.make_format(FontId::monospace((size * 0.9).max(13.0)), code_fg, false);
                    fmt.background = code_bg;
                    self.append_text(s, fmt, None, spoiler_reveal);
                }
                MarkdownSpan::HighlightedCode { language, code } => {
                    let mono = FontId::monospace((size * 0.9).max(13.0));
                    for (token, fg) in
                        inline_highlighted_runs(code, language, code_bg, code_fg, self.palette)
                    {
                        let mut fmt = self.make_format(mono.clone(), fg, false);
                        fmt.background = code_bg;
                        self.append_text(&token, fmt, None, spoiler_reveal);
                    }
                }
                MarkdownSpan::Styled { text, url, css } => {
                    self.append_styled_span(text, url.as_deref(), *css, spoiler_reveal);
                }
                MarkdownSpan::Link { label, url } => {
                    let target = self.resolve_target(url, link_fg);
                    let mut fmt = self.make_format(FontId::proportional(size), link_fg, false);
                    fmt.underline = if target.is_internal() {
                        Stroke::NONE
                    } else {
                        Stroke::new(1.0_f32, link_fg)
                    };
                    self.append_text(label, fmt, Some(target), spoiler_reveal);
                }
                MarkdownSpan::Image { alt, url } => {
                    self.append_image_placeholder(alt, url, None, spoiler_reveal);
                }
                MarkdownSpan::StyledImage { alt, url, href, .. } => {
                    self.append_image_placeholder(alt, url, href.as_deref(), spoiler_reveal);
                }
                MarkdownSpan::Math { source, .. } => {
                    let fmt = self.make_format(
                        FontId::monospace((size * 0.9).max(13.0)),
                        prose_color,
                        false,
                    );
                    self.append_text(source, fmt, None, spoiler_reveal);
                }
                MarkdownSpan::Spoiler { spans: inner } => {
                    let idx = self.spoiler_counter;
                    self.spoiler_counter += 1;
                    let revealed = self
                        .options
                        .reveal_ctx
                        .is_some_and(|(ctx, path)| spoiler_is_revealed(ctx, path, idx));
                    let start = self.char_cursor;
                    self.append_spans(inner, Some(spoiler_reveal.unwrap_or(revealed)));
                    if self.char_cursor > start {
                        self.spoilers.push((start..self.char_cursor, idx));
                    }
                }
                MarkdownSpan::HardBreak => {
                    let fmt = self.make_format(FontId::proportional(size), prose_color, false);
                    self.append_text("\n", fmt, None, spoiler_reveal);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_prose_layout_job_with_options(
    spans: &[MarkdownSpan],
    size: f32,
    prose_color: Color32,
    bold_color: Color32,
    bg_color: Color32,
    palette: &AppPalette,
    max_width: f32,
    align: egui::Align,
    options: ProseFormatOptions<'_>,
    link_cache: Option<&mut LinkCache>,
) -> BuiltProseJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = max_width;
    job.halign = align;
    let mut builder = ProseJobBuilder {
        job,
        links: Vec::new(),
        spoilers: Vec::new(),
        char_cursor: 0,
        spoiler_counter: 0,
        size,
        prose_color,
        bold_color,
        bg_color,
        code_bg: hex_to_c32(&palette.colors.surface_panel),
        code_fg: inline_code_fg(palette),
        link_fg: link_color(palette),
        muted_fg: hex_to_c32(&palette.colors.text_muted),
        palette,
        options,
        link_cache,
    };
    builder.append_spans(spans, None);
    BuiltProseJob {
        job: builder.job,
        links: builder.links,
        spoilers: builder.spoilers,
    }
}

fn hit_range_at_pos<'a, T>(
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    pointer: egui::Pos2,
    ranges: &'a [(std::ops::Range<usize>, T)],
) -> Option<&'a T> {
    let local = (pointer - galley_pos).to_pos2();
    let mut row_start = 0usize;
    for row in &galley.rows {
        let row_end = row_start + row.char_count_including_newline().0;
        if row.rect().contains(local) {
            let row_local_x = local.x - row.pos.x;
            for (idx, glyph) in row.glyphs.iter().enumerate() {
                if row_local_x >= glyph.pos.x && row_local_x < glyph.max_x() {
                    let char_idx = row_start + idx;
                    return ranges
                        .iter()
                        .find(|(range, _)| range.contains(&char_idx))
                        .map(|(_, val)| val);
                }
            }
            return None;
        }
        row_start = row_end;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn render_spans_with_options(
    ui: &mut egui::Ui,
    spans: &[MarkdownSpan],
    size: f32,
    prose_color: Color32,
    bold_color: Color32,
    bg_color: Color32,
    palette: &AppPalette,
    path: u64,
    mut media: DocMedia<'_>,
    align: egui::Align,
    options: ProseFormatOptions<'_>,
) {
    if math::needs_inline_flow(spans) {
        math::show_spans(
            ui,
            spans,
            math::SpanRenderStyle {
                size,
                prose_color,
                bold_color,
                palette,
                block: path,
            },
            &mut media,
            align,
        );
        return;
    }

    let max_width = ui.available_width().max(1.0);
    let opts = ProseFormatOptions {
        reveal_ctx: Some((ui.ctx(), path)),
        ..options
    };
    let built = build_prose_layout_job_with_options(
        spans,
        size,
        prose_color,
        bold_color,
        bg_color,
        palette,
        max_width,
        align,
        opts,
        media.links.as_deref_mut(),
    );
    let galley = ui.fonts_mut(|f| f.layout_job(built.job));
    render_galley_with_links(
        ui,
        galley,
        &built.links,
        &built.spoilers,
        path,
        media.navigate_to.as_deref_mut(),
    );
}

fn paint_dashed_underline_for_range(
    painter: &egui::Painter,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    range: &std::ops::Range<usize>,
    stroke: Stroke,
) {
    let mut row_start = 0usize;
    for row in &galley.rows {
        let row_end = row_start + row.char_count_including_newline().0;
        let from = range.start.max(row_start).min(row_end) - row_start;
        let to = (range.end.max(row_start).min(row_end) - row_start).min(row.glyphs.len());
        if from < to {
            let x0 = galley_pos.x + row.pos.x + row.x_offset(egui::text::CharIndex(from));
            let x1 = galley_pos.x + row.pos.x + row.x_offset(egui::text::CharIndex(to));
            if x1 > x0 + 0.5 {
                let first_glyph = row.glyphs[from..to]
                    .iter()
                    .find(|g| g.size().y > 0.0)
                    .or_else(|| row.glyphs[from..to].first());
                let y = if let Some(glyph) = first_glyph {
                    let offset = (glyph.size().y * 0.14).clamp(1.5, 3.0);
                    galley_pos.y + (row.pos.y + glyph.pos.y + offset).min(row.max_y() - 0.5)
                } else {
                    galley_pos.y + row.max_y() - 1.5
                };
                paint_dashed_h_line(painter, x0, x1, y, stroke, 4.0, 2.5);
            }
        }
        row_start = row_end;
    }
}

fn render_galley_with_links(
    ui: &mut egui::Ui,
    galley: std::sync::Arc<egui::Galley>,
    links: &[(std::ops::Range<usize>, LinkTarget)],
    spoilers: &[(std::ops::Range<usize>, usize)],
    path: u64,
    navigate_to: Option<&mut Option<std::path::PathBuf>>,
) {
    let halign = galley.job.halign;
    let interactive = !links.is_empty() || !spoilers.is_empty();
    let label = if interactive {
        egui::Label::new(galley.clone()).sense(egui::Sense::click())
    } else {
        egui::Label::new(galley.clone())
    };
    let response = ui.add(label);
    if !interactive {
        return;
    }
    let galley_pos = match halign {
        egui::Align::LEFT => response.rect.left_top(),
        egui::Align::Center => response.rect.center_top(),
        egui::Align::RIGHT => response.rect.right_top(),
    };
    let pointer = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos());
    let hovered_link = pointer.and_then(|pos| hit_range_at_pos(&galley, galley_pos, pos, links));
    let hovered_spoiler = pointer
        .and_then(|pos| hit_range_at_pos(&galley, galley_pos, pos, spoilers))
        .copied();

    for (range, link) in links {
        if link.is_internal() {
            let hovered = hovered_link.is_some_and(|h| std::ptr::eq(h, link));
            let stroke_w = if hovered { 1.5_f32 } else { 1.25_f32 };
            paint_dashed_underline_for_range(
                ui.painter(),
                &galley,
                galley_pos,
                range,
                Stroke::new(stroke_w, link.dash_color),
            );
        }
    }

    for &(_, idx) in spoilers {
        let hover_id = egui::Id::unique(("md_spoiler_hover", path, idx));
        let now_hovered = hovered_spoiler == Some(idx);
        let was_hovered = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(hover_id).unwrap_or(false));
        if now_hovered != was_hovered {
            ui.ctx().data_mut(|d| d.insert_temp(hover_id, now_hovered));
            ui.ctx().request_repaint();
        }
    }

    if let Some(link) = hovered_link {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if response.clicked() {
            activate_link_target(ui, link, navigate_to);
        }
        response.on_hover_text(link.hover_label());
    } else if let Some(idx) = hovered_spoiler {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if response.clicked() {
            let open_id = egui::Id::unique(("md_spoiler_open", path, idx));
            let toggled = ui
                .ctx()
                .data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
            ui.ctx().data_mut(|d| d.insert_temp(open_id, !toggled));
            ui.ctx().request_repaint();
        }
    }
}

enum MeasuredCell<'a> {
    Prose {
        galley: std::sync::Arc<egui::Galley>,
        links: Vec<(std::ops::Range<usize>, LinkTarget)>,
        spoilers: Vec<(std::ops::Range<usize>, usize)>,
        path: u64,
    },
    Math(&'a [MarkdownSpan]),
}

struct MeasuredRow<'a> {
    is_header: bool,
    cells: Vec<MeasuredCell<'a>>,
    height: f32,
}

fn map_table_alignment(table: &MarkdownTable, col_idx: usize) -> egui::Align {
    match table
        .alignments
        .get(col_idx)
        .copied()
        .unwrap_or(TableAlignment::None)
    {
        TableAlignment::Center => egui::Align::Center,
        TableAlignment::Right => egui::Align::RIGHT,
        TableAlignment::None | TableAlignment::Left => egui::Align::LEFT,
    }
}

fn table_row_background(
    is_header: bool,
    r_idx: usize,
    canvas_bg: Color32,
    palette: &AppPalette,
) -> Color32 {
    if is_header {
        hex_to_c32(&palette.colors.surface_panel)
    } else if r_idx % 2 == 1 {
        hex_to_c32(&palette.colors.surface_recessed)
    } else {
        canvas_bg
    }
}

struct TableThemeColors {
    canvas_bg: Color32,
    prose: Color32,
    bold: Color32,
}

fn measure_table_col_widths(
    ui: &egui::Ui,
    all_rows: &[(bool, &Vec<Vec<MarkdownSpan>>)],
    col_count: usize,
    avail_w: f32,
    palette: &AppPalette,
    path: u64,
    media: &mut DocMedia<'_>,
) -> (Vec<f32>, bool) {
    let canvas_bg = hex_to_c32(&palette.colors.canvas);
    let prose_color = hex_to_c32(&palette.colors.text_secondary);
    let bold_color = hex_to_c32(&palette.colors.text_primary);
    let mut natural = vec![48.0_f32; col_count];

    for (r_idx, (is_header, row)) in all_rows.iter().enumerate() {
        let color = if *is_header { bold_color } else { prose_color };
        for (c_idx, cell) in row.iter().enumerate().take(col_count) {
            let cell_w = if math::needs_inline_flow(cell) {
                let cell_path = child_path(path, r_idx * col_count + c_idx + 1);
                let mut m = media.reborrow();
                math::measure_spans_size(
                    ui,
                    cell,
                    f32::INFINITY,
                    math::SpanRenderStyle {
                        size: BODY_FONT_SIZE,
                        prose_color: color,
                        bold_color,
                        palette,
                        block: cell_path,
                    },
                    &mut m,
                )
                .x
            } else {
                let built = build_prose_layout_job_with_options(
                    cell,
                    BODY_FONT_SIZE,
                    color,
                    bold_color,
                    canvas_bg,
                    palette,
                    f32::INFINITY,
                    egui::Align::LEFT,
                    ProseFormatOptions::default(),
                    media.links.as_deref_mut(),
                );
                ui.fonts_mut(|f| f.layout_job(built.job)).size().x
            };
            natural[c_idx] = natural[c_idx].max(cell_w + TABLE_CELL_PAD_X * 2.0);
        }
    }

    let total_natural: f32 = natural.iter().sum();
    if total_natural <= avail_w {
        let widths = if col_count >= 2 && total_natural >= avail_w * 0.72 {
            let scale = avail_w / total_natural;
            natural.iter().map(|w| w * scale).collect()
        } else {
            natural
        };
        return (widths, false);
    }

    let min_cols: Vec<f32> = natural.iter().map(|w| w.min(TABLE_MIN_COL_WIDTH)).collect();
    let total_min: f32 = min_cols.iter().sum();
    if total_min > avail_w {
        let widths = natural
            .iter()
            .map(|w| w.clamp(TABLE_MIN_COL_WIDTH, TABLE_SCROLL_MAX_COL_WIDTH))
            .collect();
        return (widths, true);
    }

    let surplus = avail_w - total_min;
    let flex: Vec<f32> = natural
        .iter()
        .zip(&min_cols)
        .map(|(n, m)| (n - m).max(0.0))
        .collect();
    let total_flex: f32 = flex.iter().sum();
    let widths = if total_flex > 0.0 {
        min_cols
            .iter()
            .zip(&flex)
            .map(|(m, f)| m + surplus * (f / total_flex))
            .collect()
    } else {
        min_cols
    };
    (widths, false)
}

fn measure_table_rows<'a>(
    ui: &egui::Ui,
    table: &'a MarkdownTable,
    all_rows: &[(bool, &'a Vec<Vec<MarkdownSpan>>)],
    col_widths: &[f32],
    palette: &AppPalette,
    path: u64,
    media: &mut DocMedia<'_>,
) -> Vec<MeasuredRow<'a>> {
    let col_count = col_widths.len();
    let colors = TableThemeColors {
        canvas_bg: hex_to_c32(&palette.colors.canvas),
        prose: hex_to_c32(&palette.colors.text_secondary),
        bold: hex_to_c32(&palette.colors.text_primary),
    };
    let mut measured_rows = Vec::with_capacity(all_rows.len());

    for (r_idx, (is_header, row)) in all_rows.iter().enumerate() {
        let color = if *is_header {
            colors.bold
        } else {
            colors.prose
        };
        let row_bg = table_row_background(*is_header, r_idx, colors.canvas_bg, palette);
        let mut cells = Vec::with_capacity(col_count);
        let mut max_h = BODY_FONT_SIZE;
        for (c_idx, &col_w) in col_widths.iter().enumerate() {
            let inner_w = (col_w - TABLE_CELL_PAD_X * 2.0).max(1.0);
            let align = map_table_alignment(table, c_idx);
            let spans: &[MarkdownSpan] = row.get(c_idx).map_or(&[], Vec::as_slice);
            let cell_path = child_path(path, r_idx * col_count + c_idx + 1);
            if math::needs_inline_flow(spans) {
                let mut m = media.reborrow();
                let size = math::measure_spans_size(
                    ui,
                    spans,
                    inner_w,
                    math::SpanRenderStyle {
                        size: BODY_FONT_SIZE,
                        prose_color: color,
                        bold_color: colors.bold,
                        palette,
                        block: cell_path,
                    },
                    &mut m,
                );
                max_h = max_h.max(size.y);
                cells.push(MeasuredCell::Math(spans));
            } else {
                let built = build_prose_layout_job_with_options(
                    spans,
                    BODY_FONT_SIZE,
                    color,
                    colors.bold,
                    row_bg,
                    palette,
                    inner_w,
                    align,
                    ProseFormatOptions {
                        reveal_ctx: Some((ui.ctx(), cell_path)),
                        ..Default::default()
                    },
                    media.links.as_deref_mut(),
                );
                let galley = ui.fonts_mut(|f| f.layout_job(built.job));
                max_h = max_h.max(galley.size().y);
                cells.push(MeasuredCell::Prose {
                    galley,
                    links: built.links,
                    spoilers: built.spoilers,
                    path: cell_path,
                });
            }
        }
        measured_rows.push(MeasuredRow {
            is_header: *is_header,
            cells,
            height: max_h + TABLE_CELL_PAD_Y * 2.0,
        });
    }
    measured_rows
}

fn paint_measured_table(
    ui: &mut egui::Ui,
    table: &MarkdownTable,
    col_widths: &[f32],
    measured_rows: Vec<MeasuredRow<'_>>,
    palette: &AppPalette,
    path: u64,
    media: &mut DocMedia<'_>,
) {
    let col_count = col_widths.len();
    let table_w: f32 = col_widths.iter().sum();
    let total_h: f32 = measured_rows.iter().map(|r| r.height).sum();
    let row_count = measured_rows.len();
    let (table_rect, _) = ui.allocate_exact_size(Vec2::new(table_w, total_h), egui::Sense::hover());
    let border_outer = hex_to_c32_alpha(&palette.chrome_border, 170);
    let border_inner = hex_to_c32_alpha(&palette.chrome_border, 85);
    let canvas_bg = hex_to_c32(&palette.colors.canvas);
    let prose_color = hex_to_c32(&palette.colors.text_secondary);
    let bold_color = hex_to_c32(&palette.colors.text_primary);
    let rounding = egui::CornerRadius::from(crate::ui_ext::BADGE_CORNER_RADIUS);
    let top_corner = crate::ui_ext::BADGE_CORNER_RADIUS.round() as u8;

    ui.painter().rect_filled(table_rect, rounding, canvas_bg);

    let mut y = table_rect.top();
    for (r_idx, row) in measured_rows.into_iter().enumerate() {
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(table_rect.left(), y),
            Vec2::new(table_w, row.height),
        );
        let row_bg = table_row_background(row.is_header, r_idx, canvas_bg, palette);
        let row_rounding = if r_idx == 0 {
            egui::CornerRadius {
                nw: top_corner,
                ne: top_corner,
                sw: 0,
                se: 0,
            }
        } else {
            egui::CornerRadius::ZERO
        };
        ui.painter().rect_filled(row_rect, row_rounding, row_bg);

        let mut x = table_rect.left();
        for (c_idx, cell) in row.cells.into_iter().enumerate() {
            let col_w = col_widths[c_idx];
            let inner_w = (col_w - TABLE_CELL_PAD_X * 2.0).max(1.0);
            let align = map_table_alignment(table, c_idx);
            let cell_rect = egui::Rect::from_min_size(
                egui::pos2(x + TABLE_CELL_PAD_X, y + TABLE_CELL_PAD_Y),
                Vec2::new(inner_w, (row.height - TABLE_CELL_PAD_Y * 2.0).max(1.0)),
            );
            let layout = match align {
                egui::Align::Center => egui::Layout::top_down(egui::Align::Center),
                egui::Align::RIGHT => egui::Layout::top_down(egui::Align::RIGHT),
                _ => egui::Layout::top_down(egui::Align::Min),
            };
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(cell_rect).layout(layout),
                |ui| match cell {
                    MeasuredCell::Prose {
                        galley,
                        links,
                        spoilers,
                        path: cell_path,
                    } => {
                        render_galley_with_links(
                            ui,
                            galley,
                            &links,
                            &spoilers,
                            cell_path,
                            media.navigate_to.as_deref_mut(),
                        );
                    }
                    MeasuredCell::Math(spans) => {
                        let cell_path = child_path(path, r_idx * col_count + c_idx + 1);
                        let color = if row.is_header {
                            bold_color
                        } else {
                            prose_color
                        };
                        let mut m = media.reborrow();
                        math::show_spans(
                            ui,
                            spans,
                            math::SpanRenderStyle {
                                size: BODY_FONT_SIZE,
                                prose_color: color,
                                bold_color,
                                palette,
                                block: cell_path,
                            },
                            &mut m,
                            align,
                        );
                    }
                },
            );

            x += col_w;
            if c_idx + 1 < col_count {
                ui.painter().line_segment(
                    [
                        egui::pos2(x, row_rect.top()),
                        egui::pos2(x, row_rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, border_inner),
                );
            }
        }

        y += row.height;
        if r_idx + 1 < row_count {
            let divider_stroke = if row.is_header {
                Stroke::new(1.5_f32, border_outer)
            } else {
                Stroke::new(1.0_f32, border_inner)
            };
            ui.painter().line_segment(
                [
                    egui::pos2(table_rect.left(), y),
                    egui::pos2(table_rect.right(), y),
                ],
                divider_stroke,
            );
        }
    }

    ui.painter().rect_stroke(
        table_rect,
        rounding,
        Stroke::new(1.0_f32, border_outer),
        egui::StrokeKind::Inside,
    );
}

fn render_markdown_table(
    ui: &mut egui::Ui,
    table: &MarkdownTable,
    palette: &AppPalette,
    path: u64,
    mut media: DocMedia<'_>,
) {
    let col_count = table
        .alignments
        .len()
        .max(table.headers.len())
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let avail_w = ui.available_width().max(1.0);
    let all_rows: Vec<(bool, &Vec<Vec<MarkdownSpan>>)> = (!table.headers.is_empty())
        .then_some((true, &table.headers))
        .into_iter()
        .chain(table.rows.iter().map(|r| (false, r)))
        .collect();

    let (col_widths, needs_h_scroll) =
        measure_table_col_widths(ui, &all_rows, col_count, avail_w, palette, path, &mut media);

    let draw_table = |ui: &mut egui::Ui, media: &mut DocMedia<'_>| {
        let measured_rows =
            measure_table_rows(ui, table, &all_rows, &col_widths, palette, path, media);
        paint_measured_table(ui, table, &col_widths, measured_rows, palette, path, media);
    };

    if needs_h_scroll {
        egui::ScrollArea::horizontal()
            .id_salt(("markdown_table_h", path))
            .max_width(avail_w)
            .show(ui, |ui| draw_table(ui, &mut media));
    } else {
        draw_table(ui, &mut media);
    }
}

/// Renders a styled code block for markdown view.
pub(super) fn render_styled_code_block(
    ui: &mut egui::Ui,
    code: &str,
    lang: &str,
    palette: &AppPalette,
    path: u64,
) {
    let syntax = find_syntax(lang);
    let display_code = code.strip_suffix('\n').unwrap_or(code);

    egui::Frame::NONE
        .fill(hex_to_c32(&palette.colors.surface_recessed))
        .stroke(Stroke::new(
            1.0_f32,
            hex_to_c32(&palette.colors.surface_panel),
        ))
        .inner_margin(egui::Margin::same(10))
        .corner_radius(egui::CornerRadius::from(crate::ui_ext::CONTROL_CORNER_RADIUS))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let label = if lang.is_empty() {
                    "CODE".to_owned()
                } else {
                    lang.to_uppercase()
                };
                ui.label(
                    egui::RichText::new(label)
                        .size(13.5)
                        .color(hex_to_c32(&palette.colors.text_muted)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            clip_ui::buttons::ButtonRole::Secondary
                                .button("Copy", palette)
                                .compact(),
                        )
                        .clicked()
                    {
                        ui.ctx().copy_text(display_code.to_owned());
                        #[cfg(not(test))]
                        if let Err(error) =
                            little_clip_core::copy_text_to_system_clipboard(display_code)
                        {
                            tracing::debug!(%error, "failed to copy code block to system clipboard");
                        }
                    }
                });
            });
            ui.add_space(crate::ui_ext::SPACE_XS);

            let avail_w = ui.available_width().max(1.0);
            egui::ScrollArea::horizontal()
                .id_salt(("markdown_code_h", path))
                .max_width(avail_w)
                .show(ui, |ui| {
                    let mut layouter =
                        |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap_width: f32| {
                            let text = buffer.as_str();
                            let mut layout_job =
                                build_highlighted_layout_job(text, syntax, palette, &[]);
                            layout_job.wrap.max_width = f32::INFINITY;
                            ui.fonts_mut(|f| f.layout_job(layout_job))
                        };

                    let mut code_buf = display_code;
                    let text_edit = egui::TextEdit::multiline(&mut code_buf)
                        .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
                        .layouter(&mut layouter)
                        .frame(egui::Frame::NONE)
                        .desired_width(avail_w)
                        .lock_focus(false)
                        .interactive(true);

                    ui.add(text_edit);
                });
        });
}
