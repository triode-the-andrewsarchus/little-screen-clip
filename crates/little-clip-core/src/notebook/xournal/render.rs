//! Selected-page geometry is retained as SVG until the requested raster size.
use super::*;
use base64::Engine;
use resvg::usvg::{TreeParsing, TreeTextToPath};
use std::{io::Cursor, path::Path, sync::LazyLock};

mod background;
mod elements;

type Node<'a> = roxmltree::Node<'a, 'a>;
const MAX_SVG: usize = 32 * 1024 * 1024;

/// Renders a selected Xournal page using native SVG paths and bundled fonts.
///
/// `source_path` supplies the original directory for guarded local backgrounds.
/// Call in a disposable worker with the same checked input/path identity.
///
/// # Errors
/// Returns container, page, geometry, resource, rendering or cancellation errors.
pub fn render(
    input: &mut (impl Read + Seek),
    source_path: &Path,
    page: usize,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), String> {
    if !(1..=4096).contains(&edge) {
        return Err("Invalid notebook preview size".into());
    }
    let document = inspect(input, cancel)?;
    let sheet = page
        .checked_sub(1)
        .and_then(|index| document.pages.get(index))
        .ok_or("Xournal page is outside the notebook")?;
    let tree = parse(&document.xml)?;
    let pages = tree
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("page"))
        .collect::<Vec<_>>();
    let node = pages[page - 1];
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' width='{}' height='{}' viewBox='0 0 {} {}'>",
        sheet.width, sheet.height, sheet.width, sheet.height
    );
    let background = background::draw(
        &mut svg,
        node,
        &pages,
        input,
        source_path,
        document.packaged,
        edge,
        cancel,
    )?;
    draw_layers(
        &mut svg,
        node,
        input,
        document.packaged,
        f64::from(edge) / sheet.width.max(sheet.height),
        cancel,
    )?;
    svg.push_str("</svg>");
    budget(&svg)?;
    check_cancel(cancel)?;
    let image = rasterize(&svg, edge, cancel)?;
    if let Some(background) = background {
        background.validate()?;
    }
    Ok(image)
}

fn draw_layers(
    svg: &mut String,
    node: Node<'_>,
    input: &mut (impl Read + Seek),
    packaged: bool,
    scale: f64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    for layer in node.children().filter(|node| node.has_tag_name("layer")) {
        svg.push_str("<g>");
        for element in layer.children().filter(roxmltree::Node::is_element) {
            check_cancel(cancel)?;
            elements::draw(svg, element, input, packaged, scale, cancel)?;
            budget(svg)?;
        }
        svg.push_str("</g>");
    }
    Ok(())
}

fn rasterize(svg: &str, edge: u32, cancel: &AtomicBool) -> Result<(Vec<u8>, u32, u32), String> {
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
    check_cancel(cancel)?;
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

fn budget(svg: &str) -> Result<(), String> {
    if svg.len() > MAX_SVG {
        Err("Xournal page exceeds the geometry budget".into())
    } else {
        Ok(())
    }
}

fn number(value: &str) -> Result<f64, String> {
    let value: f64 = value.parse().map_err(|_| "Invalid Xournal number")?;
    if !value.is_finite() || value.abs() > 1_000_000.0 {
        return Err("Xournal number exceeds the coordinate budget".into());
    }
    Ok(value)
}

fn attr(node: Node<'_>, key: &str) -> Result<f64, String> {
    number(
        node.attribute(key)
            .ok_or_else(|| format!("Missing Xournal {key}"))?,
    )
}

fn numbers(value: &str) -> Result<Vec<f64>, String> {
    value
        .split_whitespace()
        .take(1_000_001)
        .map(number)
        .collect::<Result<Vec<_>, _>>()
        .and_then(|values| {
            if values.len() > 1_000_000 {
                Err("Xournal point budget exceeded".into())
            } else {
                Ok(values)
            }
        })
}

fn color(value: &str) -> Result<(String, f64), String> {
    let value = match value {
        "black" => "#000000",
        "white" => "#ffffff",
        "blue" => "#0000ff",
        "red" => "#ff0000",
        "green" => "#008000",
        "gray" | "grey" => "#808080",
        "lightblue" => "#c0e0ff",
        "lightgreen" => "#80ff80",
        "magenta" => "#ff00ff",
        "cyan" => "#00ffff",
        "yellow" => "#ffff00",
        "orange" => "#ff8000",
        "darkblue" => "#000080",
        other => other,
    };
    let hex = value.strip_prefix('#').ok_or("Unsupported Xournal color")?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid Xournal color".into());
    }
    let alpha = if hex.len() == 8 {
        u8::from_str_radix(&hex[6..], 16).map_err(|error| error.to_string())?
    } else {
        255
    };
    Ok((format!("#{}", &hex[..6]), f64::from(alpha) / 255.0))
}

fn escaped(value: &str) -> std::borrow::Cow<'_, str> {
    quick_xml::escape::escape(value)
}

fn raster(bytes: &[u8]) -> Result<(String, u32, u32), String> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("Xournal image exceeds its byte budget".into());
    }
    let format = image::guess_format(bytes).map_err(|error| error.to_string())?;
    if !format.reading_enabled() {
        return Err("Unsupported Xournal raster image format".into());
    }
    let size = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    if size.0 == 0 || size.1 == 0 || u64::from(size.0) * u64::from(size.1) > 16 * 1024 * 1024 {
        return Err("Xournal image exceeds the pixel budget".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| error.to_string())?;
    let mut png = Cursor::new(Vec::new());
    decoded
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok((
        base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
        size.0,
        size.1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_pages_render_against_independent_cli_exports() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/notebooks/xournal");
        let cases: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("reference/reference-exports.json")).unwrap(),
        )
        .unwrap();
        let mut failures = Vec::new();
        for case in cases.as_array().unwrap() {
            let name = case["source"].as_str().unwrap();
            let page = case["page"].as_u64().unwrap() as usize;
            let path = root.join(name);
            let result = render(
                &mut std::fs::File::open(&path).unwrap(),
                &path,
                page,
                800,
                &AtomicBool::new(false),
            );
            let (png, width, height) = match result {
                Ok(image) => image,
                Err(error) => {
                    failures.push(format!("{name}: {error}"));
                    continue;
                }
            };
            assert!(width <= 800 && height <= 800);
            if let Some(directory) = std::env::var_os("XOURNAL_REVIEW_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(
                    std::path::PathBuf::from(directory).join(case["image"].as_str().unwrap()),
                    &png,
                )
                .unwrap();
            }
            let actual = image::load_from_memory(&png).unwrap().to_rgb8();
            let reference =
                image::open(root.join("reference").join(case["image"].as_str().unwrap()))
                    .unwrap()
                    .resize_exact(width, height, image::imageops::FilterType::Triangle)
                    .to_rgb8();
            let error = actual
                .as_raw()
                .iter()
                .zip(reference.as_raw())
                .map(|(a, b)| f64::from(a.abs_diff(*b)))
                .sum::<f64>()
                / actual.as_raw().len() as f64;
            let foreground = |image: &image::RgbImage| {
                image
                    .pixels()
                    .filter(|pixel| pixel.0.iter().copied().min().unwrap() < 225)
                    .count()
            };
            let ratio = foreground(&actual) as f64 / foreground(&reference).max(1) as f64;
            eprintln!("{name} page {page}: mean RGB error {error:.3}, foreground ratio {ratio:.3}");
            if error > 8.0 || !(0.65..=1.6).contains(&ratio) {
                failures.push(format!(
                    "{name}: mean error {error:.3}, foreground ratio {ratio:.3}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn invalid_geometry_missing_backgrounds_and_cancellation_never_become_blank_success() {
        let source = Path::new("/tmp/nonexistent-notebook-fixture.xopp");
        for body in [
            "<stroke tool='pen' width='1'>0 0 10</stroke>",
            "<stroke tool='pen' width='NaN'>0 0 10 10</stroke>",
            "<text font='Sans' size='12' matrix='1 0 0'>text</text>",
            "<image left='0' top='0' right='10' bottom='10'>bad base64</image>",
        ] {
            let xml = format!(
                "<xournal><page width='100' height='100'><background type='solid' color='white' style='plain'/><layer>{body}</layer></page></xournal>"
            );
            assert!(
                render(
                    &mut Cursor::new(xml),
                    source,
                    1,
                    100,
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
        let xml = "<xournal><page width='100' height='100'><background type='pdf' domain='absolute' filename='https://example.invalid/file.pdf' pageno='1'/><layer/></page></xournal>";
        assert!(
            render(
                &mut Cursor::new(xml),
                source,
                1,
                100,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .contains("URLs")
        );
        assert!(
            render(
                &mut Cursor::new(xml),
                source,
                1,
                100,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert!(
            render(
                &mut Cursor::new(xml),
                source,
                0,
                100,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
}
