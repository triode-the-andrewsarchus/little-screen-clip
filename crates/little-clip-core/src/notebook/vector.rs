//! Native notebook SVG rasterization with bundled fonts and no file/URL resolver.
use crate::page_region::PageRegion;
use base64::Engine;
use resvg::{
    tiny_skia::{self as skia, Pixmap, Transform},
    usvg::{self, Group, ImageKind, Node},
};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::io::Cursor;
use std::sync::{Arc, LazyLock, atomic::AtomicBool};

pub(crate) fn rasterize(
    svg: &str,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), String> {
    let tree = parse(svg)?;
    let scale = edge as f32 / tree.size().width().max(tree.size().height());
    let width = (tree.size().width() * scale)
        .round()
        .clamp(1.0, edge as f32) as u32;
    let height = (tree.size().height() * scale)
        .round()
        .clamp(1.0, edge as f32) as u32;
    let mut pixmap = white_canvas(width, height)?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    super::check_cancel(cancel)?;
    Ok((
        pixmap.encode_png().map_err(|error| error.to_string())?,
        width,
        height,
    ))
}

pub(super) fn parse(svg: &str) -> Result<usvg::Tree, String> {
    let options = usvg::Options {
        font_family: "Arimo".into(),
        fontdb: Arc::clone(fonts()),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..Default::default()
        },
        ..Default::default()
    };
    usvg::Tree::from_str(svg, &options).map_err(|error| error.to_string())
}

pub(super) fn page_svg(width: u32, height: u32) -> String {
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' width='{width}' height='{height}'>"
    )
}

pub(super) fn append_raster(
    svg: &mut String,
    page: [u32; 2],
    image: image::RgbaImage,
) -> Result<(), String> {
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    append_png(svg, page, &png.into_inner());
    Ok(())
}

pub(super) fn append_png(svg: &mut String, [width, height]: [u32; 2], png: &[u8]) {
    let _ = write!(
        svg,
        "<image width='{width}' height='{height}' preserveAspectRatio='none' xlink:href='data:image/png;base64,"
    );
    base64::engine::general_purpose::STANDARD.encode_string(png, svg);
    svg.push_str("'/>");
}

fn white_canvas(width: u32, height: u32) -> Result<Pixmap, String> {
    let mut pixmap = Pixmap::new(width, height).ok_or("Notebook raster allocation failed")?;
    pixmap.fill(skia::Color::WHITE);
    Ok(pixmap)
}

/// One notebook page with its prepared geometry and separately decoded raster assets.
pub struct RetainedPage {
    tree: usvg::Tree,
    assets: HashMap<usize, Pixmap>,
    image_groups: HashSet<usize>,
}

impl RetainedPage {
    pub(super) fn new(svg: &str, cancel: &AtomicBool) -> Result<Self, String> {
        let tree = parse(svg)?;
        let mut assets = HashMap::new();
        let mut image_groups = HashSet::new();
        let mut remaining_pixels = 32 * 1024 * 1024;
        decode_images(
            tree.root(),
            &mut assets,
            &mut image_groups,
            &mut remaining_pixels,
            cancel,
        )?;
        Ok(Self {
            tree,
            assets,
            image_groups,
        })
    }

    /// Draws a physical-pixel window with the original vector paths and asset order.
    ///
    /// # Errors
    /// Returns cancellation or an invalid window/allocation error.
    pub fn render_region(&self, region: PageRegion, cancel: &AtomicBool) -> Result<Pixmap, String> {
        region.validate()?;
        super::check_cancel(cancel)?;
        let [width, height] = region.size;
        let [x, y] = region.origin;
        let transform = Transform::from_row(
            region.page[0] as f32 / self.tree.size().width(),
            0.0,
            0.0,
            region.page[1] as f32 / self.tree.size().height(),
            -(x as f32),
            -(y as f32),
        );
        let mut pixmap = white_canvas(width, height)?;
        if self.assets.is_empty() {
            resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        } else {
            self.paint(self.tree.root(), transform, &mut pixmap, cancel)?;
        }
        super::check_cancel(cancel)?;
        Ok(pixmap)
    }

    fn paint(
        &self,
        group: &Group,
        transform: Transform,
        canvas: &mut Pixmap,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let viewport =
            skia::Rect::from_xywh(0.0, 0.0, canvas.width() as f32, canvas.height() as f32)
                .ok_or("Invalid notebook detail canvas")?;
        for node in group.children() {
            super::check_cancel(cancel)?;
            let Some(bounds) = node.abs_layer_bounding_box() else {
                continue;
            };
            if bounds
                .to_rect()
                .transform(transform)
                .and_then(|bounds| bounds.intersect(&viewport))
                .is_none()
            {
                continue;
            }
            match node {
                Node::Group(child) if self.image_groups.contains(&group_key(child)) => {
                    self.paint(child, transform, canvas, cancel)?;
                }
                Node::Image(image) if image.is_visible() => {
                    let ImageKind::PNG(data) = image.kind() else {
                        continue;
                    };
                    if let Some(asset) = self.assets.get(&image_key(data)) {
                        let paint = skia::PixmapPaint {
                            quality: skia::FilterQuality::Bicubic,
                            ..Default::default()
                        };
                        canvas.draw_pixmap(
                            0,
                            0,
                            asset.as_ref(),
                            &paint,
                            transform.pre_concat(image.abs_transform()),
                            None,
                        );
                    }
                }
                _ => {
                    // resvg draws a subtree in parent coordinates after subtracting
                    // its absolute bounds. Undo that offset to retain page placement
                    // and let it handle isolated highlighters, text, and all vector ink.
                    let parent = transform.pre_concat(group.abs_transform());
                    resvg::render_node(
                        node,
                        parent.pre_translate(bounds.x(), bounds.y()),
                        &mut canvas.as_mut(),
                    );
                }
            }
        }
        Ok(())
    }
}

fn decode_images(
    group: &Group,
    assets: &mut HashMap<usize, Pixmap>,
    image_groups: &mut HashSet<usize>,
    remaining_pixels: &mut u32,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let mut contains_image = false;
    for node in group.children() {
        super::check_cancel(cancel)?;
        match node {
            Node::Group(child) => {
                contains_image |=
                    decode_images(child, assets, image_groups, remaining_pixels, cancel)?;
            }
            Node::Image(image) if image.is_visible() => {
                let ImageKind::PNG(data) = image.kind() else {
                    return Err("Unexpected format for a prepared Xournal asset".into());
                };
                let pixels =
                    (image.size().width() as u32).saturating_mul(image.size().height() as u32);
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    assets.entry(image_key(data))
                {
                    *remaining_pixels = remaining_pixels
                        .checked_sub(pixels)
                        .ok_or("Xournal raster assets exceed the retained pixel budget")?;
                    entry.insert(Pixmap::decode_png(data).map_err(|error| error.to_string())?);
                }
                contains_image = true;
            }
            _ => {}
        }
    }
    if contains_image && !group.should_isolate() {
        image_groups.insert(group_key(group));
        Ok(true)
    } else {
        Ok(false)
    }
}

fn image_key(data: &Arc<Vec<u8>>) -> usize {
    Arc::as_ptr(data) as usize
}

fn group_key(group: &Group) -> usize {
    std::ptr::from_ref(group) as usize
}

#[cfg(test)]
pub(super) fn verify_crops(scene: &RetainedPage, svg: &str, cancel: &AtomicBool, name: &str) {
    let tree = parse(svg).unwrap();
    let full = [
        tree.size().width().round() as u32,
        tree.size().height().round() as u32,
    ];
    let mut overview = white_canvas(full[0], full[1]).unwrap();
    resvg::render(&tree, Transform::identity(), &mut overview.as_mut());
    let has_detail =
        |pixel: &skia::PremultipliedColorU8| pixel.red().min(pixel.green()).min(pixel.blue()) < 180;
    let ink = overview
        .pixels()
        .iter()
        .position(has_detail)
        .expect("fixture contains visible details") as u32;
    for (scale, origin, size) in [
        (1, [0, 0], full),
        (
            12,
            [
                (ink % full[0] * 12).saturating_sub(200),
                (ink / full[0] * 12).saturating_sub(200),
            ],
            [420, 380],
        ),
    ] {
        let page = full.map(|side| side * scale);
        let origin = std::array::from_fn(|axis| origin[axis].min(page[axis] - size[axis]));
        let region = PageRegion { page, origin, size };
        let mut expected = white_canvas(size[0], size[1]).unwrap();
        resvg::render(
            &tree,
            Transform::from_row(
                page[0] as f32 / tree.size().width(),
                0.0,
                0.0,
                page[1] as f32 / tree.size().height(),
                -(origin[0] as f32),
                -(origin[1] as f32),
            ),
            &mut expected.as_mut(),
        );
        let actual = scene.render_region(region, cancel).unwrap();
        let total_error: usize = actual
            .data()
            .iter()
            .zip(expected.data())
            .map(|(a, b)| a.abs_diff(*b) as usize)
            .sum();
        let mae = total_error as f64 / expected.data().len() as f64;
        assert!(mae < 0.5, "{name}, {scale}×: MAE {mae}");
        assert!(actual.pixels().iter().any(has_detail));
    }
}

fn fonts() -> &'static Arc<resvg::usvg::fontdb::Database> {
    static FONTS: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
        let mut fonts = resvg::usvg::fontdb::Database::new();
        for bytes in crate::font_assets::GRAPHVIZ_FONT_BYTES {
            fonts.load_font_source(resvg::usvg::fontdb::Source::Binary(Arc::new(bytes)));
        }
        fonts.load_font_source(resvg::usvg::fontdb::Source::Binary(Arc::new(
            crate::font_assets::CJK_FONT_BYTES,
        )));
        fonts.set_sans_serif_family("Arimo");
        fonts.set_serif_family("Tinos");
        fonts.set_monospace_family("Cousine");
        Arc::new(fonts)
    });
    &FONTS
}
