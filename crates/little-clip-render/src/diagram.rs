//! Compiled diagram engines, isolated from hosts by their existing executable.
use little_clip_core::{CommandRequest, diagram::Format};
use resvg::usvg::{TreeParsing, TreeTextToPath};
use std::{
    io::{Read, Write},
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod mermaid;

const MAX_SOURCE: usize = 256 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_SVG: usize = 2 * 1024 * 1024;
const WIDTH: u32 = 1536;
const HEIGHT: u32 = 1024;
const WORKER_FLAG: &str = "--internal-diagram-worker";

#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    format: Format,
    source: String,
}

/// An immutable rendered diagram, retaining vectors for export and source inspection.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rendered {
    pub svg: String,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Renders diagram text in a bounded, owned copy of the current host executable.
///
/// Call this from a preview worker, never a UI callback. Cancellation kills/reaps
/// the process group; no diagram source can choose a command or resolve resources.
/// The host must dispatch [`run_worker_cli`] before ordinary startup.
///
/// # Errors
/// Returns source-budget, admission/spawn, cancellation, parse/render or protocol errors.
pub fn render(format: Format, source: &str, cancelled: &AtomicBool) -> Result<Rendered, String> {
    validate_source(source)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Diagram preview cancelled".into());
    }
    let request = serde_json::to_vec(&Request {
        format,
        source: source.to_owned(),
    })
    .map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let output = CommandRequest::new(executable.into_os_string())
        .arg(WORKER_FLAG)
        .stdin_bytes(request)
        .timeout(Duration::from_secs(8))
        .resource_limit(rustix::process::Resource::As, 2 * 1024 * 1024 * 1024)
        .resource_limit(rustix::process::Resource::Cpu, 6)
        .resource_limit(rustix::process::Resource::Fsize, MAX_OUTPUT as u64)
        .max_output_bytes(MAX_OUTPUT)
        .own_process_group()
        .die_with_parent()
        .run_cancellable(cancelled)
        .map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(format!(
            "Diagram worker failed: {}",
            output.stderr_lossy().chars().take(512).collect::<String>()
        ));
    }
    let result: Result<Rendered, String> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Invalid diagram worker response: {error}"))?;
    let rendered = result.map_err(|error| error.chars().take(4096).collect::<String>())?;
    if rendered.width == 0
        || rendered.height == 0
        || rendered.width > WIDTH
        || rendered.height > HEIGHT
        || rendered.svg.len() > MAX_SVG
        || rendered.png.len() > MAX_OUTPUT / 2
    {
        return Err("Diagram worker exceeded output limits".into());
    }
    Ok(rendered)
}

/// Dispatches a compiled diagram helper before application/runtime initialization.
///
/// Unrelated arguments return `false`. The helper reads bounded JSON on stdin,
/// writes its result on stdout and exits; it never opens source/output filenames.
/// `--diagram-licenses` prints the bundled Mermaid notices instead.
pub fn run_worker_cli() -> bool {
    let argument = std::env::args_os().nth(1);
    if argument.as_deref() == Some(std::ffi::OsStr::new("--diagram-licenses")) {
        print!(
            "{}\n{}\n{}",
            include_str!("../third_party/mermaid-notices.md"),
            include_str!("../third_party/merman-MIT.txt"),
            include_str!("../third_party/merman-elk-EPL-2.0.txt"),
        );
        return true;
    }
    if argument.as_deref() != Some(std::ffi::OsStr::new(WORKER_FLAG)) {
        return false;
    }
    let response = (|| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((MAX_SOURCE * 8 + 1024) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let request: Request = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        render_inline(request.format, &request.source)
    })();
    if let Err(error) = serde_json::to_writer(std::io::stdout().lock(), &response)
        .and_then(|_| std::io::stdout().flush().map_err(serde_json::Error::io))
    {
        eprintln!("Diagram response failed: {error}");
        std::process::exit(2);
    }
    true
}

fn validate_source(source: &str) -> Result<(), String> {
    if source.len() > MAX_SOURCE || source.contains('\0') {
        return Err("Diagram source exceeds 256 KiB or contains a NUL byte".into());
    }
    Ok(())
}

fn render_inline(format: Format, source: &str) -> Result<Rendered, String> {
    validate_source(source)?;
    let svg = match format {
        Format::Mermaid => mermaid::svg(source)?,
        Format::Pikchr => pikchr_rs::pikchr(
            source,
            pikchr_rs::PikchrFlags {
                plaintext_errors: true,
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())?,
        Format::Svgbob => {
            if source.lines().count() > 128 || source.lines().any(|line| line.chars().count() > 256)
            {
                return Err("SvgBob preview exceeds the 256 × 128 cell limit".into());
            }
            svgbob::to_svg(source)
        }
    };
    rasterize_svg(svg)
}

fn rasterize_svg(svg: String) -> Result<Rendered, String> {
    if svg.len() > MAX_SVG {
        return Err("Diagram SVG exceeds 2 MiB".into());
    }
    let document = roxmltree::Document::parse(&svg).map_err(|error| error.to_string())?;
    if document.descendants().count() > 16_384
        || document
            .descendants()
            .any(|node| node.ancestors().take(66).count() > 64)
    {
        return Err("Diagram SVG exceeds structural limits".into());
    }
    let mut options = resvg::usvg::Options {
        font_family: "Noto Sans CJK JP".into(),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    // Responsive SVGs use percentages for their root size. Their viewBox,
    // rather than usvg's 100×100 fallback, supplies the native viewport.
    if let Some(viewbox) = document.root_element().attribute("viewBox") {
        let values = viewbox
            .split(|value: char| value.is_ascii_whitespace() || value == ',')
            .filter(|value| !value.is_empty())
            .take(5)
            .map(str::parse::<f32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Invalid diagram viewBox: {error}"))?;
        if values.len() != 4
            || values.iter().any(|value| !value.is_finite())
            || values[2] > 16_384.0
            || values[3] > 16_384.0
        {
            return Err("Diagram viewBox exceeds dimension limits".into());
        }
        options.default_size = resvg::usvg::Size::from_wh(values[2], values[3])
            .ok_or("Diagram viewBox has non-positive dimensions")?;
    }
    let mut tree =
        resvg::usvg::Tree::from_str(&svg, &options).map_err(|error| error.to_string())?;
    if tree.size.width() > 16_384.0 || tree.size.height() > 16_384.0 {
        return Err("Diagram dimensions exceed 16384 pixels".into());
    }
    static FONTS: LazyLock<resvg::usvg::fontdb::Database> = LazyLock::new(|| {
        let mut database = resvg::usvg::fontdb::Database::new();
        database.load_font_data(little_clip_core::font_assets::CJK_FONT_BYTES.to_vec());
        database.load_font_data(little_clip_core::font_assets::EMOJI_FONT_BYTES.to_vec());
        database.set_sans_serif_family("Noto Sans CJK JP");
        database.set_serif_family("Noto Sans CJK JP");
        database.set_monospace_family("Noto Sans CJK JP");
        database
    });
    tree.convert_text(&FONTS);
    let scale = (WIDTH as f32 / tree.size.width())
        .min(HEIGHT as f32 / tree.size.height())
        .min(2.0);
    let width = ((tree.size.width() * scale).ceil().max(1.0) as u32).min(WIDTH);
    let height = ((tree.size.height() * scale).ceil().max(1.0) as u32).min(HEIGHT);
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or("Diagram raster allocation failed")?;
    // White paper preserves renderer-defined colors in either application theme.
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::Tree::from_usvg(&tree).render(
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let png = pixmap.encode_png().map_err(|error| error.to_string())?;
    Ok(Rendered {
        svg,
        png,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responsive_svg_keeps_its_viewbox_aspect_ratio() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="100%" viewBox="0 0 400 100"><rect width="400" height="100" fill="black"/></svg>"#;
        let rendered = rasterize_svg(svg.into()).unwrap();
        assert_eq!((rendered.width, rendered.height), (800, 200));
    }

    #[test]
    fn markdown_files_and_raw_diagram_files_use_the_screen_diagram_route() {
        use little_clip_core::{ClipItem, ClipPayload, SlotManager};
        let directory = tempfile::tempdir().unwrap();
        for (extension, content, format, source) in [
            ("md", "```pikchr\nbox\n```", Format::Pikchr, "box\n"),
            ("pikchr", "box\n", Format::Pikchr, "box\n"),
            (
                "md",
                "```mermaid\nflowchart LR\nA-->B\n```",
                Format::Mermaid,
                "flowchart LR\nA-->B\n",
            ),
            (
                "mmd",
                "flowchart LR\nA-->B\n",
                Format::Mermaid,
                "flowchart LR\nA-->B\n",
            ),
            (
                "mermaid",
                "flowchart LR\nA-->B\n",
                Format::Mermaid,
                "flowchart LR\nA-->B\n",
            ),
            ("mmd", "# Doc\n```pikchr\nbox\n```", Format::Pikchr, "box\n"),
        ] {
            let path = directory.path().join(format!("drawing.{extension}"));
            std::fs::write(&path, content).unwrap();
            let renderer = crate::ScreenRenderer::fallback(Default::default());
            let image = image::RgbaImage::from_pixel(32, 16, image::Rgba([255, 0, 0, 255]));
            renderer
                .diagram_previews
                .lock()
                .unwrap()
                .0
                .push_back(Cached {
                    format,
                    source: source.into(),
                    result: Ok(std::sync::Arc::new(
                        little_clip_core::ImageThumbnail::from_rgba(image),
                    )),
                    prepared: std::time::Instant::now(),
                });
            let mut item = ClipItem::from_text("drawing");
            item.payload = ClipPayload::FileBatch(vec![path]);
            let mut slots = SlotManager::new();
            slots.push_left(item);
            let frame = renderer.render_pixmap(&slots).unwrap();
            let red = frame
                .pixels()
                .iter()
                .filter(|pixel| pixel.red() > 240 && pixel.green() < 10 && pixel.blue() < 10)
                .count();
            assert!(
                red > 100,
                "{extension} must paint the prepared diagram, not source code"
            );
        }
    }

    #[test]
    fn native_engines_render_vectors_and_bounded_readable_rasters() {
        for (format, source) in [
            (Format::Pikchr, "box \"Read\"; arrow; circle \"Think\""),
            (Format::Mermaid, "flowchart LR\nA[Source] --> B[Preview]"),
            (
                Format::Svgbob,
                "+------+     +------+\n| Read +---->| Think|\n+------+     +------+",
            ),
        ] {
            let rendered = render_inline(format, source).unwrap();
            assert!(rendered.svg.contains("<svg"));
            let pixels = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
            assert_eq!(pixels.dimensions(), (rendered.width, rendered.height));
            assert!(
                pixels
                    .pixels()
                    .any(|pixel| pixel[0] < 128 && pixel[1] < 128)
            );
            assert!(rendered.width <= WIDTH && rendered.height <= HEIGHT);
        }
    }

    #[test]
    fn malformed_inputs_and_resource_exhaustion_are_errors() {
        assert!(render_inline(Format::Pikchr, "box at invalid nonsense").is_err());
        assert!(render_inline(Format::Pikchr, "\0").is_err());
        assert!(render_inline(Format::Pikchr, &"x".repeat(MAX_SOURCE + 1)).is_err());
        assert!(render_inline(Format::Svgbob, &"+".repeat(257)).is_err());
        assert!(render_inline(Format::Svgbob, &"x\n".repeat(129)).is_err());
    }
}

struct Cached {
    format: Format,
    source: String,
    result: Result<std::sync::Arc<little_clip_core::ImageThumbnail>, String>,
    prepared: std::time::Instant,
}

#[derive(Default)]
pub(crate) struct Cache(std::collections::VecDeque<Cached>);
impl Cache {
    pub(crate) fn thumbnail(
        &mut self,
        format: Format,
        source: &str,
    ) -> Result<std::sync::Arc<little_clip_core::ImageThumbnail>, String> {
        if let Some(index) = self.0.iter().position(|entry| {
            entry.format == format
                && entry.source == source
                && ((entry.result.is_ok()
                    && (format != Format::Mermaid
                        || entry.prepared.elapsed() < Duration::from_secs(60)))
                    || (entry.result.is_err()
                        && entry.prepared.elapsed() < Duration::from_secs(10)))
        }) {
            let entry = self.0.remove(index).ok_or("Diagram cache changed")?;
            let result = entry.result.clone();
            self.0.push_back(entry);
            return result;
        }
        validate_source(source)?;
        self.0
            .retain(|entry| entry.format != format || entry.source != source);
        let result = render(format, source, &AtomicBool::new(false)).and_then(|rendered| {
            little_clip_core::ImageThumbnail::generate(&rendered.png, WIDTH, HEIGHT)
                .map(std::sync::Arc::new)
                .ok_or_else(|| "Invalid diagram raster".into())
        });
        if self.0.len() >= 8 {
            self.0.pop_front();
        }
        self.0.push_back(Cached {
            format,
            source: source.to_owned(),
            result: result.clone(),
            prepared: std::time::Instant::now(),
        });
        result
    }
}
