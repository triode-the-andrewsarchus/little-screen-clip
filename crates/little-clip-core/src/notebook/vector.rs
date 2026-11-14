//! Native notebook SVG rasterization with bundled fonts and no file/URL resolver.
use resvg::usvg::{TreeParsing, TreeTextToPath};
use std::sync::{
    LazyLock,
    atomic::{AtomicBool, Ordering},
};

pub(crate) fn rasterize(
    svg: &str,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), String> {
    let options = resvg::usvg::Options {
        font_family: "Arimo".into(),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|error| error.to_string())?;
    tree.convert_text(fonts());
    let scale = edge as f32 / tree.size.width().max(tree.size.height());
    let width = (tree.size.width() * scale).round().clamp(1.0, edge as f32) as u32;
    let height = (tree.size.height() * scale).round().clamp(1.0, edge as f32) as u32;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width, height).ok_or("Notebook raster allocation failed")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::Tree::from_usvg(&tree).render(
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

fn fonts() -> &'static resvg::usvg::fontdb::Database {
    static FONTS: LazyLock<resvg::usvg::fontdb::Database> = LazyLock::new(|| {
        let mut fonts = resvg::usvg::fontdb::Database::new();
        for bytes in crate::font_assets::GRAPHVIZ_FONT_BYTES {
            fonts.load_font_data(bytes.to_vec());
        }
        fonts.load_font_data(crate::font_assets::CJK_FONT_BYTES.to_vec());
        fonts.set_sans_serif_family("Arimo");
        fonts.set_serif_family("Tinos");
        fonts.set_monospace_family("Cousine");
        fonts
    });
    &FONTS
}
