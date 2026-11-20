//! Native notebook SVG rasterization with bundled fonts and no file/URL resolver.
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, Ordering},
};

pub(crate) fn rasterize(
    svg: &str,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), String> {
    let options = resvg::usvg::Options {
        font_family: "Arimo".into(),
        fontdb: Arc::clone(fonts()),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..Default::default()
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|error| error.to_string())?;
    let scale = edge as f32 / tree.size().width().max(tree.size().height());
    let width = (tree.size().width() * scale)
        .round()
        .clamp(1.0, edge as f32) as u32;
    let height = (tree.size().height() * scale)
        .round()
        .clamp(1.0, edge as f32) as u32;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width, height).ok_or("Notebook raster allocation failed")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    if cancel.load(Ordering::Relaxed) {
        return Err("Notebook preview cancelled".into());
    }
    Ok((
        pixmap.encode_png().map_err(|error| error.to_string())?,
        width,
        height,
    ))
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
