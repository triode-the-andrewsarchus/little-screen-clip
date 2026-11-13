//! Compiled diagram engines, isolated from hosts by their existing executable.
use little_clip_core::{
    CommandRequest,
    diagram::{Format, MAX_DRAWING_SOURCE_BYTES as MAX_DRAWING_SOURCE},
};
use resvg::usvg::{TreeParsing, TreeTextToPath};
use std::{
    io::{Read, Write},
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod bpmn;
mod bytefield;
mod d2;
mod dbml;
mod ditaa;
mod erd;
mod excalidraw;
mod fonts;
mod goat;
mod graphviz;
mod images;
mod java;
pub mod retained;
mod umlet;
use images::Policy as ImagePolicy;
mod blockdiag;
mod drawio;
mod lini;
mod math;
mod mermaid;
mod nomnoml;
mod plantuml;
mod python;
mod script;
mod structurizr;
mod symbolator;
mod tikz;
mod vega;
mod wavedrom;
mod wireviz;

/// Owned fixtures for testing early dispatch in independently linked hosts.
#[cfg(feature = "test-support")]
pub mod test_support;

const MAX_SOURCE: usize = 256 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_SVG: usize = 2 * 1024 * 1024;
const WIDTH: u32 = 1536;
const HEIGHT: u32 = 1024;
const WORKER_FLAG: &str = "--internal-diagram-worker";
const MEMORY_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const CPU_LIMIT: u64 = 6;

#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    format: Format,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    foreground: Option<[u8; 3]>,
}

/// Natural equation dimensions relative to the surrounding text's font size.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MathLayout {
    pub width_em: f32,
    pub height_em: f32,
    /// Distance from the top edge to the text baseline, in em.
    pub baseline_em: f32,
}
impl MathLayout {
    fn valid(self) -> bool {
        [self.width_em, self.height_em, self.baseline_em]
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0 && *value <= 512.0)
            && self.baseline_em <= self.height_em
    }
}

/// Source-space viewport for a drawing's normalized, outlined preview.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrawingLayout {
    pub origin: [f64; 2],
    pub size: [f64; 2],
}
impl DrawingLayout {
    fn valid(self) -> bool {
        self.origin
            .iter()
            .all(|value| value.is_finite() && value.abs() <= 1e12)
            && self
                .size
                .iter()
                .all(|value| value.is_finite() && *value > 0.0 && *value <= 16384.0)
    }
}

/// An immutable rendered diagram, retaining vectors for export and source inspection.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rendered {
    pub svg: String,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Non-fatal renderer limitations, retained alongside the image.
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub math: Option<MathLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drawing: Option<DrawingLayout>,
}

impl Rendered {
    fn validate(&self, format: Format) -> Result<(), String> {
        let expects_math = matches!(format, Format::Math | Format::MathInline);
        if self.math.is_some() != expects_math || self.math.is_some_and(|layout| !layout.valid()) {
            return Err("Invalid equation layout in worker response".into());
        }
        if self.drawing.is_some() != (format == Format::Excalidraw)
            || self.drawing.is_some_and(|layout| !layout.valid())
        {
            return Err("Invalid drawing viewport in worker response".into());
        }
        if self.width == 0
            || self.height == 0
            || self.width > WIDTH
            || self.height > HEIGHT
            || self.svg.len() > MAX_SVG
            || self.png.len() > MAX_OUTPUT / 2
            || self.warnings.len() > 16
            || self
                .warnings
                .iter()
                .any(|warning| warning.chars().count() > 1024)
        {
            return Err("Diagram worker exceeded output limits".into());
        }
        Ok(())
    }
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
    render_request(format, source, None, cancelled)
}

/// Renders a transparent equation with native text metrics and the requested ink.
///
/// Run on a preview worker, like [`render`]. Explicit LaTeX colors are preserved.
///
/// # Errors
/// Returns the same bounded worker and source errors as [`render`].
pub fn render_math(
    source: &str,
    display: bool,
    foreground: [u8; 3],
    cancelled: &AtomicBool,
) -> Result<Rendered, String> {
    render_request(
        if display {
            Format::Math
        } else {
            Format::MathInline
        },
        source,
        Some(foreground),
        cancelled,
    )
}

fn render_request(
    format: Format,
    source: &str,
    foreground: Option<[u8; 3]>,
    cancelled: &AtomicBool,
) -> Result<Rendered, String> {
    validate_source(format, source)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Diagram preview cancelled".into());
    }
    let request = serde_json::to_vec(&Request {
        format,
        source: source.to_owned(),
        foreground,
    })
    .map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let output = CommandRequest::new(executable.into_os_string())
        .arg(WORKER_FLAG)
        .stdin_bytes(request)
        .timeout(Duration::from_secs(8))
        .resource_limit(rustix::process::Resource::As, MEMORY_LIMIT)
        .resource_limit(rustix::process::Resource::Cpu, CPU_LIMIT)
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
    rendered.validate(format)?;
    Ok(rendered)
}

/// Dispatches a compiled diagram helper before application/runtime initialization.
///
/// Unrelated arguments return `false`. The helper reads bounded JSON on stdin,
/// writes its result on stdout and exits; it never opens source/output filenames.
/// `--diagram-licenses` prints the bundled renderer notices instead.
pub fn run_worker_cli() -> bool {
    let argument = std::env::args_os().nth(1);
    if argument.as_deref() == Some(std::ffi::OsStr::new("--diagram-licenses")) {
        for notice in [
            include_str!("../third_party/diagram-notices.md"),
            include_str!("../third_party/bpmnkit-MIT.txt"),
            include_str!("../third_party/wavedrom-notices.md"),
            include_str!("../third_party/excalidraw-notices.md"),
            include_str!("../third_party/excalidraw-render-LICENSE-BUNDLE.txt"),
            include_str!("../third_party/excalidraw-document-LICENSE-MIT.txt"),
            include_str!("../third_party/excalidraw-document-LICENSE-APACHE.txt"),
            include_str!("../../../vendor/wavedrom/LICENSE"),
            include_str!("../../../vendor/nomnoml/LICENSE"),
            include_str!("../../../vendor/goat/LICENSE"),
            include_str!("../third_party/bytefield-notices.md"),
            include_str!("../third_party/erd-notices.md"),
            include_str!("../third_party/plantuml-notices.md"),
            include_str!("../third_party/drawio-notices.md"),
            include_str!("../third_party/structurizr-notices.md"),
            include_str!("../third_party/vega-notices.md"),
            include_str!("../../../vendor/tikra/LICENSE-MIT"),
            include_str!(
                "../../../vendor/tikra/third_party/mathra-fonts-GUST-FONT-LICENSE-LatinModernMath.txt"
            ),
            include_str!("../../../vendor/tikra/third_party/mathra-fonts-README.md"),
            include_str!("../third_party/mathra-lppl-1.3c.txt"),
            include_str!("../../../vendor/tikra/ADAPTATIONS.md"),
            include_str!("../../../vendor/symbolator/upstream/LICENSE"),
            include_str!("../../../vendor/symbolator/ADAPTATIONS.md"),
            include_str!("diagram/blockdiag/parser-MIT.txt"),
            include_str!("../../../vendor/blockdiag-family/blockdiag/LICENSE"),
            include_str!("../../../vendor/blockdiag-family/seqdiag/LICENSE"),
            include_str!("../../../vendor/blockdiag-family/actdiag/LICENSE"),
            include_str!("../../../vendor/blockdiag-family/nwdiag/LICENSE"),
            include_str!("../../../vendor/blockdiag-family/funcparserlib/LICENSE"),
            include_str!("../../../vendor/blockdiag-family/webcolors/LICENSE"),
            include_str!("../../../vendor/ditaa/licenses/GPL-3.0.txt"),
            include_str!("../../../vendor/ditaa/upstream/LICENSE"),
            include_str!("../../../vendor/umlet/upstream/LICENCE.txt"),
            include_str!("../../../vendor/umlet/jlibeps-pristine/license.txt"),
            include_str!("../../../vendor/ditaa/ADAPTATIONS.md"),
            include_str!("../../../vendor/umlet/ADAPTATIONS.md"),
            include_str!("../../../vendor/structurizr-rs/LICENSE-MIT"),
            include_str!("../../../vendor/document-svg-drawio/LICENSE-MIT"),
            include_str!("../../../vendor/document-svg-drawio/LICENSE-APACHE"),
            include_str!("../../../vendor/plantuml-core/LICENSE"),
            include_str!("../../../vendor/plantuml-core/stdlib/c4/LICENSE"),
            include_str!("../../../vendor/erd-reference/UNLICENSE"),
            include_str!("../../../vendor/bytefield-svg/LICENSE"),
            include_str!("../../../vendor/bytefield-svg/licenses/sci-LICENSE"),
            include_str!("../../../vendor/bytefield-svg/licenses/clojurescript-LICENSE"),
            include_str!("../../../vendor/bytefield-svg/licenses/tools.reader-LICENSE"),
            include_str!("../../../vendor/bytefield-svg/licenses/closure-LICENSE"),
            include_str!("../../../vendor/tdewolff-parse/LICENSE.md"),
            include_str!("../third_party/go-runtime-LICENSE.txt"),
            include_str!("../third_party/dbml-rs-MIT.txt"),
            include_str!("../third_party/dbml-rs-APACHE.txt"),
            include_str!("../../../vendor/graphre/LICENSE"),
            include_str!("../third_party/wavedrom-onml-LICENSE"),
            include_str!("../third_party/wavedrom-tspan-LICENSE"),
            include_str!("../third_party/wavedrom-logidrom-LICENSE.md"),
            include_str!("../third_party/wavedrom-bit-field-LICENSE"),
            include_str!("../third_party/rquickjs-MIT.txt"),
            include_str!("../third_party/quickjs-MIT.txt"),
            include_str!("../third_party/json-five-MIT.txt"),
            include_str!("../third_party/json5-MIT.txt"),
            include_str!("../third_party/merman-MIT.txt"),
            include_str!("../third_party/merman-elk-EPL-2.0.txt"),
            include_str!("../third_party/lini-MIT.txt"),
            include_str!("../third_party/lini-google-sans-OFL.txt"),
            include_str!("../third_party/lini-google-sans-code-OFL.txt"),
            include_str!("../third_party/lini-phosphor-MIT.txt"),
            include_str!("../third_party/d2-notices.txt"),
            include_str!("../third_party/d2-upstream-notices.md"),
            include_str!("../third_party/d2-mathjax-NOTICE.txt"),
            include_str!("../third_party/mathra-MIT.txt"),
            include_str!("../third_party/mathra-stix-OFL.txt"),
            include_str!("../third_party/graphviz-EPL-2.0.txt"),
            include_str!("../third_party/graphviz-NOTICE.txt"),
            include_str!("../third_party/graphviz-core-math-MIT.txt"),
            include_str!("../third_party/graphviz-expat-MIT.txt"),
            include_str!("../assets/fonts/graphviz/OFL-Tinos.txt"),
            include_str!("../assets/fonts/graphviz/OFL-Arimo.txt"),
            include_str!("../assets/fonts/graphviz/OFL-Cousine.txt"),
        ] {
            println!("{notice}");
        }
        return true;
    }
    if argument.as_deref() != Some(std::ffi::OsStr::new(WORKER_FLAG)) {
        return false;
    }
    let response = (|| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((MAX_DRAWING_SOURCE * 8 + 1024) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let request: Request = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        render_inline(request.format, &request.source, request.foreground)
    })();
    if let Err(error) = serde_json::to_writer(std::io::stdout().lock(), &response)
        .and_then(|_| std::io::stdout().flush().map_err(serde_json::Error::io))
    {
        eprintln!("Diagram response failed: {error}");
        std::process::exit(2);
    }
    true
}

// All adapters share the transport/display budget, including overflow reporting.
fn bounded_warnings(warnings: Vec<String>) -> Vec<String> {
    let omitted = warnings.len().saturating_sub(15);
    let mut bounded: Vec<_> = warnings
        .into_iter()
        .take(15)
        .map(|warning| warning.chars().take(1024).collect())
        .collect();
    if omitted > 0 {
        bounded.push(format!("{omitted} additional diagram warnings omitted"));
    }
    bounded
}

fn validate_source(format: Format, source: &str) -> Result<(), String> {
    let limit = if format == Format::Excalidraw {
        MAX_DRAWING_SOURCE
    } else {
        MAX_SOURCE
    };
    if source.len() > limit || source.contains('\0') {
        return Err(format!(
            "Diagram source exceeds {} KiB or contains a NUL byte",
            limit / 1024
        ));
    }
    Ok(())
}

fn render_inline(
    format: Format,
    source: &str,
    foreground: Option<[u8; 3]>,
) -> Result<Rendered, String> {
    validate_source(format, source)?;
    let mut math_layout = None;
    let (svg, warnings) = match format {
        Format::D2 => return d2::render(source),
        Format::Excalidraw => return excalidraw::render(source),
        Format::Math | Format::MathInline => {
            let (svg, layout) = math::svg(
                source,
                format == Format::Math,
                foreground.unwrap_or([17; 3]),
            )?;
            math_layout = Some(layout);
            (svg, Vec::new())
        }
        Format::Wavedrom => wavedrom::svg(source)?,
        Format::Nomnoml => nomnoml::svg(source)?,
        Format::Dbml => dbml::svg(source)?,
        Format::Goat => goat::svg(source)?,
        Format::Bytefield => bytefield::svg(source)?,
        Format::Erd => erd::svg(source)?,
        Format::Plantuml => plantuml::svg(source)?,
        Format::Drawio => drawio::svg(source)?,
        Format::Structurizr => structurizr::svg(source)?,
        Format::Vega => vega::svg(source, false)?,
        Format::VegaLite => vega::svg(source, true)?,
        Format::Blockdiag => blockdiag::svg("blockdiag", source)?,
        Format::Seqdiag => blockdiag::svg("seqdiag", source)?,
        Format::Actdiag => blockdiag::svg("actdiag", source)?,
        Format::Nwdiag => blockdiag::svg("nwdiag", source)?,
        Format::Packetdiag => blockdiag::svg("packetdiag", source)?,
        Format::Rackdiag => blockdiag::svg("rackdiag", source)?,
        Format::Ditaa => ditaa::svg(source)?,
        Format::Tikz => tikz::svg(source)?,
        Format::Umlet => umlet::svg(source)?,
        Format::Symbolator => symbolator::svg(source)?,
        Format::Wireviz => wireviz::svg(source)?,
        Format::Bpmn => bpmn::svg(source)?,
        Format::Graphviz => graphviz::svg(source)?,
        Format::Lini => lini::svg(source)?,
        Format::Mermaid => (mermaid::svg(source)?, Vec::new()),
        Format::Pikchr => (
            pikchr_rs::pikchr(
                source,
                pikchr_rs::PikchrFlags {
                    plaintext_errors: true,
                    ..Default::default()
                },
            )
            .map_err(|error| error.to_string())?,
            Vec::new(),
        ),
        Format::Svgbob => {
            if source.lines().count() > 128 || source.lines().any(|line| line.chars().count() > 256)
            {
                return Err("SvgBob preview exceeds the 256 × 128 cell limit".into());
            }
            (svgbob::to_svg(source), Vec::new())
        }
    };
    let images = if matches!(format, Format::Plantuml | Format::Drawio) {
        ImagePolicy::EmbeddedRaster
    } else {
        ImagePolicy::Block
    };
    let mut rendered = rasterize_svg(svg, math_layout.is_none(), images)?;
    rendered.math = math_layout;
    rendered.warnings.extend(warnings);
    rendered.warnings = bounded_warnings(rendered.warnings);
    Ok(rendered)
}

fn default_fonts() -> &'static resvg::usvg::fontdb::Database {
    static FONTS: LazyLock<resvg::usvg::fontdb::Database> = LazyLock::new(|| {
        let mut database = resvg::usvg::fontdb::Database::new();
        database.load_font_data(little_clip_core::font_assets::CJK_FONT_BYTES.to_vec());
        database.load_font_data(little_clip_core::font_assets::EMOJI_FONT_BYTES.to_vec());
        database.set_sans_serif_family("Noto Sans CJK JP");
        database.set_serif_family("Noto Sans CJK JP");
        database.set_monospace_family("Noto Sans CJK JP");
        database
    });
    &FONTS
}

fn parse_svg(
    svg: &str,
    fonts: &resvg::usvg::fontdb::Database,
    family: &str,
    images: ImagePolicy,
) -> Result<(resvg::usvg::Tree, Vec<String>), String> {
    let document = svg_document(svg)?;
    let (image_href_resolver, image_budget) = images::resolver(images);
    let mut options = resvg::usvg::Options {
        font_family: family.into(),
        image_href_resolver,
        ..Default::default()
    };
    // Responsive SVGs use percentages for their root size. Their viewBox,
    // rather than usvg's 100×100 fallback, supplies the native viewport.
    if let Some(viewbox) = svg_viewbox(document.root_element())? {
        options.default_size = resvg::usvg::Size::from_wh(viewbox.width(), viewbox.height())
            .ok_or("Diagram viewBox has non-positive dimensions")?;
    }
    let mut tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|error| error.to_string())?;
    if tree.size.width() > 16_384.0 || tree.size.height() > 16_384.0 {
        return Err("Diagram dimensions exceed 16384 pixels".into());
    }
    tree.convert_text(fonts);
    let has_images = document
        .descendants()
        .any(|node| node.has_tag_name("image"));
    let warnings = match images {
        ImagePolicy::Block if has_images => vec!["Linked or embedded images are not displayed in diagram previews".into()],
        ImagePolicy::EmbeddedRaster if image_budget.rejected.load(Ordering::Relaxed) || images::invalid_declarations(&document) => vec!["Some images were omitted: only bounded embedded PNG, JPEG and WebP rasters are displayed".into()],
        _ => Vec::new(),
    };
    Ok((tree, warnings))
}

fn svg_document(svg: &str) -> Result<roxmltree::Document<'_>, String> {
    if svg.len() > MAX_SVG {
        return Err("Diagram SVG exceeds 2 MiB".into());
    }
    let document = roxmltree::Document::parse(svg).map_err(|error| error.to_string())?;
    if document.descendants().count() > 16_384
        || document
            .descendants()
            .any(|node| node.ancestors().take(66).count() > 64)
    {
        return Err("Diagram SVG exceeds structural limits".into());
    }
    Ok(document)
}

fn svg_viewbox(root: roxmltree::Node<'_, '_>) -> Result<Option<resvg::usvg::NonZeroRect>, String> {
    let Some(viewbox) = root.attribute("viewBox") else {
        return Ok(None);
    };
    let values = viewbox
        .split(|value: char| value.is_ascii_whitespace() || value == ',')
        .filter(|value| !value.is_empty())
        .take(5)
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Invalid diagram viewBox: {error}"))?;
    let [x, y, width, height] = values.as_slice() else {
        return Err("Diagram viewBox needs four coordinates".into());
    };
    if values.iter().any(|value| !value.is_finite()) || *width > 16_384.0 || *height > 16_384.0 {
        return Err("Diagram viewBox exceeds dimension limits".into());
    }
    resvg::usvg::NonZeroRect::from_xywh(*x, *y, *width, *height)
        .map(Some)
        .ok_or_else(|| "Diagram viewBox has non-positive dimensions".into())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewportPolicy {
    FitContent,
    Preserve,
}

// Outline and namespace each document before stacking, so marker, gradient and
// clipping IDs cannot cross between independently authored diagrams.
fn compose_svg(
    diagrams: impl IntoIterator<Item = String>,
    fonts: &resvg::usvg::fontdb::Database,
    family: &str,
    images: ImagePolicy,
    viewport_policy: ViewportPolicy,
) -> Result<(String, Vec<String>), String> {
    use resvg::usvg::{NodeExt, TreeWriting};
    use std::fmt::Write;
    let mut body = String::new();
    let mut warnings = Vec::new();
    let (mut width, mut height) = (0.0_f32, 0.0_f32);
    for (index, svg) in diagrams.into_iter().enumerate() {
        if index >= 16 {
            return Err("Previews support at most sixteen diagram planes".into());
        }
        let (mut tree, limitations) = parse_svg(&svg, fonts, family, images)?;
        // Some generators size the root from shapes alone, excluding external
        // labels. Text has been outlined, so fit its real glyph geometry before
        // nesting rather than clipping labels or guessing a fixed text margin.
        // Authored native viewports already account for image/frame clips.
        // NodeExt's geometric bbox includes invisible portions of clipped images.
        if viewport_policy == ViewportPolicy::FitContent
            && let Some(bounds) = tree.root.calculate_bbox()
        {
            let old = tree.view_box.rect;
            let viewport = resvg::usvg::NonZeroRect::from_ltrb(
                old.left().min(bounds.left() - 2.0),
                old.top().min(bounds.top() - 2.0),
                old.right().max(bounds.right() + 2.0),
                old.bottom().max(bounds.bottom() + 2.0),
            )
            .ok_or("Invalid diagram content bounds")?;
            if viewport.width() > 16_384.0 || viewport.height() > 16_384.0 {
                return Err("Diagram labels exceed the viewport budget".into());
            }
            tree.size = resvg::usvg::Size::from_wh(
                tree.size.width() * viewport.width() / old.width(),
                tree.size.height() * viewport.height() / old.height(),
            )
            .ok_or("Invalid diagram content size")?;
            tree.view_box.rect = viewport;
        }
        warnings.extend(limitations);
        width = width.max(tree.size.width());
        let normalized = tree.to_string(&resvg::usvg::XmlOptions {
            id_prefix: Some(format!("graph-{index}-")),
            ..Default::default()
        });
        write!(
            body,
            "<g transform=\"translate(0 {height})\">{normalized}</g>"
        )
        .map_err(|error| error.to_string())?;
        height += tree.size.height() + 16.0;
        if height > 16_384.0 || body.len() > MAX_SVG {
            return Err("Combined diagrams exceed the viewport budget".into());
        }
    }
    if height == 0.0 {
        return Err("No diagram planes to render".into());
    }
    height -= 16.0;
    Ok((
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">{body}</svg>"
        ),
        warnings,
    ))
}

fn rasterize_svg(svg: String, white_paper: bool, images: ImagePolicy) -> Result<Rendered, String> {
    let (tree, warnings) = parse_svg(&svg, default_fonts(), "Noto Sans CJK JP", images)?;
    let scale = (WIDTH as f32 / tree.size.width())
        .min(HEIGHT as f32 / tree.size.height())
        .min(2.0);
    let width = ((tree.size.width() * scale).ceil().max(1.0) as u32).min(WIDTH);
    let height = ((tree.size.height() * scale).ceil().max(1.0) as u32).min(HEIGHT);
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or("Diagram raster allocation failed")?;
    // White paper preserves renderer-defined colors in either application theme.
    if white_paper {
        pixmap.fill(tiny_skia::Color::WHITE);
    }
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
        warnings,
        math: None,
        drawing: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_cache_keeps_theme_identity_and_screen_rows_reserve_fraction_height() {
        use crate::markdown::parse_markdown_spans;
        let renderer = crate::ScreenRenderer::fallback(Default::default());
        let foreground = [30, 210, 90];
        let color = tiny_skia::Color::from_rgba8(30, 210, 90, 255);
        let source = r"\frac{1}{\frac{x}{y}}";
        let rendered = render_inline(Format::MathInline, source, Some(foreground)).unwrap();
        let thumbnail = std::sync::Arc::new(Thumbnail {
            math: rendered.math,
            image: little_clip_core::ImageThumbnail::generate(&rendered.png, WIDTH, HEIGHT)
                .unwrap(),
            warnings: Vec::new(),
        });
        let mut cache = renderer.diagram_previews.lock().unwrap();
        for (foreground, result) in [
            (Some(foreground), Ok(thumbnail.clone())),
            (Some([200; 3]), Err("Other theme".into())),
        ] {
            cache.0.push_back(Cached {
                format: Format::MathInline,
                source: source.into(),
                foreground,
                result,
                prepared: std::time::Instant::now(),
            });
        }
        assert!(std::sync::Arc::ptr_eq(
            &thumbnail,
            &cache
                .thumbnail_colored(Format::MathInline, source, Some(foreground))
                .unwrap()
        ));
        assert_eq!(
            cache
                .thumbnail_colored(Format::MathInline, source, Some([200; 3]))
                .unwrap_err(),
            "Other theme"
        );
        drop(cache);
        let spans = parse_markdown_spans(&format!("Value ${source}$ end"));
        let mut pixmap = tiny_skia::Pixmap::new(500, 180).unwrap();
        let height = renderer.draw_markdown_spans(
            &mut pixmap,
            &spans,
            10.0,
            10.0,
            20.0,
            color,
            490.0,
            150.0,
            Some(("• ", color)),
        );
        assert!(
            height > 32.0,
            "fraction needs more than an ordinary row: {height}"
        );
        let painted_bottom = pixmap
            .pixels()
            .chunks(500)
            .rposition(|row| row.iter().any(|pixel| pixel.alpha() > 0))
            .unwrap();
        assert!((painted_bottom as f32) < 10.0 + height);
        assert!(
            pixmap
                .pixels()
                .iter()
                .any(|p| p.alpha() > 150 && p.green() > 150 && p.red() < 60)
        );
    }

    #[test]
    fn responsive_svg_keeps_its_viewbox_aspect_ratio() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="100%" viewBox="0 0 400 100"><rect width="400" height="100" fill="black"/></svg>"#;
        let rendered = rasterize_svg(svg.into(), true, ImagePolicy::Block).unwrap();
        assert_eq!((rendered.width, rendered.height), (800, 200));
    }

    #[test]
    fn markdown_files_and_raw_diagram_files_use_the_screen_diagram_route() {
        use little_clip_core::{ClipItem, ClipPayload, SlotManager};
        const VEGA: &str = include_str!("../../../docs/examples/diagrams/native-vega.vega.json");
        const LITE: &str = include_str!("../../../docs/examples/diagrams/native-vega-lite.vl.json");
        const STRUCTURIZR: &str =
            include_str!("../../../docs/examples/diagrams/architecture.structurizr");
        const DRAWIO: &str = include_str!("../../../docs/examples/diagrams/pipeline.drawio");
        const PUML: &str = include_str!("../../../docs/examples/diagrams/sequence.puml");
        const C4: &str = include_str!("../../../docs/examples/diagrams/architecture.puml");
        const ERD: &str = include_str!("../../../docs/examples/diagrams/people.er");
        const BYTEFIELD: &str = include_str!("../../../docs/examples/diagrams/packet.bytefield");
        const GOAT: &str = include_str!("../../../docs/examples/diagrams/pipeline.goat");
        const SCHEMA: &str = include_str!("../../../docs/examples/diagrams/schema.dbml");
        const HARNESS: &str = include_str!("../../../docs/examples/diagrams/harness.wireviz");
        let directory = tempfile::tempdir().unwrap();
        for (extension, content, format, source) in [
            (
                "",
                "[Source] -> [Preview]",
                Format::Nomnoml,
                "[Source] -> [Preview]",
            ),
            (
                "nomnoml",
                "[Source] -> [Preview]",
                Format::Nomnoml,
                "[Source] -> [Preview]",
            ),
            (
                "md",
                "```nomnoml\n[Source] -> [Preview]\n```",
                Format::Nomnoml,
                "[Source] -> [Preview]\n",
            ),
            (
                "",
                "```nomnoml\n[Source] -> [Preview]\n```",
                Format::Nomnoml,
                "[Source] -> [Preview]\n",
            ),
            ("", VEGA, Format::Vega, VEGA),
            ("vega", VEGA, Format::Vega, VEGA),
            ("vg.json", VEGA, Format::Vega, VEGA),
            ("", LITE, Format::VegaLite, LITE),
            ("vegalite", LITE, Format::VegaLite, LITE),
            ("vl.json", LITE, Format::VegaLite, LITE),
            ("", STRUCTURIZR, Format::Structurizr, STRUCTURIZR),
            ("structurizr", STRUCTURIZR, Format::Structurizr, STRUCTURIZR),
            ("dsl", STRUCTURIZR, Format::Structurizr, STRUCTURIZR),
            ("", DRAWIO, Format::Drawio, DRAWIO),
            ("drawio", DRAWIO, Format::Drawio, DRAWIO),
            ("dio", DRAWIO, Format::Drawio, DRAWIO),
            ("xml", DRAWIO, Format::Drawio, DRAWIO),
            ("", PUML, Format::Plantuml, PUML),
            ("puml", PUML, Format::Plantuml, PUML),
            ("", C4, Format::Plantuml, C4),
            (
                "md",
                "```c4plantuml\n@startuml\nA -> B\n@enduml\n```",
                Format::Plantuml,
                "@startuml\nA -> B\n@enduml\n",
            ),
            ("", ERD, Format::Erd, ERD),
            ("er", ERD, Format::Erd, ERD),
            ("erd", ERD, Format::Erd, ERD),
            (
                "md",
                include_str!("../../../docs/examples/diagrams/erd.md"),
                Format::Erd,
                ERD,
            ),
            ("", BYTEFIELD, Format::Bytefield, BYTEFIELD),
            ("bytefield", BYTEFIELD, Format::Bytefield, BYTEFIELD),
            ("edn", BYTEFIELD, Format::Bytefield, BYTEFIELD),
            (
                "md",
                include_str!("../../../docs/examples/diagrams/bytefield.md"),
                Format::Bytefield,
                BYTEFIELD,
            ),
            ("", GOAT, Format::Goat, GOAT),
            ("goat", GOAT, Format::Goat, GOAT),
            (
                "md",
                include_str!("../../../docs/examples/diagrams/goat.md"),
                Format::Goat,
                GOAT,
            ),
            ("", SCHEMA, Format::Dbml, SCHEMA),
            ("dbml", SCHEMA, Format::Dbml, SCHEMA),
            (
                "",
                include_str!("../../../docs/examples/diagrams/dbml.md"),
                Format::Dbml,
                SCHEMA,
            ),
            (
                "md",
                include_str!("../../../docs/examples/diagrams/dbml.md"),
                Format::Dbml,
                SCHEMA,
            ),
            ("", HARNESS, Format::Wireviz, HARNESS),
            ("wireviz", HARNESS, Format::Wireviz, HARNESS),
            ("wire.yaml", HARNESS, Format::Wireviz, HARNESS),
            ("yaml", HARNESS, Format::Wireviz, HARNESS),
            (
                "md",
                include_str!("../../../docs/examples/diagrams/harness.md"),
                Format::Wireviz,
                HARNESS,
            ),
            (
                "",
                "flowchart LR\nA-->B",
                Format::Mermaid,
                "flowchart LR\nA-->B",
            ),
            (
                "",
                "digraph { a -> b }",
                Format::Graphviz,
                "digraph { a -> b }",
            ),
            (
                "",
                "box \"Read\"; arrow; circle",
                Format::Pikchr,
                "box \"Read\"; arrow; circle",
            ),
            ("", "|box| \"Source\"", Format::Lini, "|box| \"Source\""),
            (
                "",
                "direction: right\na -> b",
                Format::D2,
                "direction: right\na -> b",
            ),
            (
                "",
                "+----+\n|Hi! |\n+----+",
                Format::Svgbob,
                "+----+\n|Hi! |\n+----+",
            ),
            (
                "",
                "<definitions xmlns='http://www.omg.org/spec/BPMN/20100524/MODEL'/>",
                Format::Bpmn,
                "<definitions xmlns='http://www.omg.org/spec/BPMN/20100524/MODEL'/>",
            ),
            (
                "",
                "```mermaid\nflowchart LR\nA-->B\n```",
                Format::Mermaid,
                "flowchart LR\nA-->B\n",
            ),
            (
                "",
                "box \"file.example\"; arrow; circle",
                Format::Pikchr,
                "box \"file.example\"; arrow; circle",
            ),
            (
                "md",
                "```wavedrom\n{signal:[{wave:'01'}]}\n```",
                Format::Wavedrom,
                "{signal:[{wave:'01'}]}\n",
            ),
            (
                "json5",
                "{signal:[{name:'clk',wave:'p...'}]}",
                Format::Wavedrom,
                "{signal:[{name:'clk',wave:'p...'}]}",
            ),
            (
                "wavejson",
                "{signal:[{wave:'01'}]}",
                Format::Wavedrom,
                "{signal:[{wave:'01'}]}",
            ),
            (
                "",
                "{signal:[{name:'clk',wave:'p...'}]}",
                Format::Wavedrom,
                "{signal:[{name:'clk',wave:'p...'}]}",
            ),
            (
                "excalidraw",
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
                Format::Excalidraw,
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
            ),
            (
                "json",
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
                Format::Excalidraw,
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
            ),
            (
                "",
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
                Format::Excalidraw,
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
            ),
            (
                "md",
                concat!(
                    "```excalidraw\n",
                    r#"{"type":"excalidraw","version":2,"elements":[]}"#,
                    "\n```"
                ),
                Format::Excalidraw,
                concat!(r#"{"type":"excalidraw","version":2,"elements":[]}"#, "\n"),
            ),
            ("md", "```d2\na -> b\n```", Format::D2, "a -> b\n"),
            ("d2", "a -> b\n", Format::D2, "a -> b\n"),
            (
                "md",
                "```bpmn\n<definitions/>\n```",
                Format::Bpmn,
                "<definitions/>\n",
            ),
            ("bpmn", "<definitions/>", Format::Bpmn, "<definitions/>"),
            (
                "xml",
                "<definitions xmlns='http://www.omg.org/spec/BPMN/20100524/MODEL'/>",
                Format::Bpmn,
                "<definitions xmlns='http://www.omg.org/spec/BPMN/20100524/MODEL'/>",
            ),
            ("md", "$x$", Format::MathInline, "x"),
            ("md", "$$x$$", Format::Math, "x"),
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
            (
                "md",
                "```graphviz\ndigraph { a -> b }\n```",
                Format::Graphviz,
                "digraph { a -> b }\n",
            ),
            (
                "gv",
                "digraph { a -> b }\n",
                Format::Graphviz,
                "digraph { a -> b }\n",
            ),
            (
                "dot",
                "digraph { a -> b }\n",
                Format::Graphviz,
                "digraph { a -> b }\n",
            ),
            (
                "graphviz",
                "digraph { a -> b }\n",
                Format::Graphviz,
                "digraph { a -> b }\n",
            ),
            (
                "md",
                "```lini\n|box| \"Source\"\n```",
                Format::Lini,
                "|box| \"Source\"\n",
            ),
            (
                "lini",
                "|box| \"Source\"\n",
                Format::Lini,
                "|box| \"Source\"\n",
            ),
        ] {
            let path = directory.path().join(format!("drawing.{extension}"));
            if !extension.is_empty() {
                std::fs::write(&path, content).unwrap();
            }
            let renderer = crate::ScreenRenderer::fallback(Default::default());
            let math = matches!(format, Format::Math | Format::MathInline);
            let color = renderer.color_from_hex(&renderer.palette.colors.text_muted);
            let foreground = math.then(|| {
                [color.red(), color.green(), color.blue()]
                    .map(|value| (value * 255.0).round() as u8)
            });
            let image = image::RgbaImage::from_pixel(32, 16, image::Rgba([255, 0, 0, 255]));
            renderer
                .diagram_previews
                .lock()
                .unwrap()
                .0
                .push_back(Cached {
                    format,
                    foreground,
                    source: source.into(),
                    result: Ok(std::sync::Arc::new(Thumbnail {
                        image: little_clip_core::ImageThumbnail::from_rgba(image),
                        warnings: Vec::new(),
                        math: math.then_some(MathLayout {
                            width_em: 4.0,
                            height_em: 1.0,
                            baseline_em: 0.8,
                        }),
                    })),
                    prepared: std::time::Instant::now(),
                });
            let mut item = ClipItem::from_text(content);
            if !extension.is_empty() {
                item.payload = ClipPayload::FileBatch(vec![path]);
            }
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
            (Format::Lini, "|box#a| \"Read\"\n|box#b| \"Think\"\na -> b"),
            (Format::Graphviz, "digraph { a [label=\"Source\"]; a -> b }"),
            (
                Format::Svgbob,
                "+------+     +------+\n| Read +---->| Think|\n+------+     +------+",
            ),
        ] {
            let rendered = render_inline(format, source, None).unwrap();
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
        assert!(render_inline(Format::Lini, "|box", None).is_err());
        assert!(render_inline(Format::Pikchr, "box at invalid nonsense", None).is_err());
        assert!(render_inline(Format::Pikchr, "\0", None).is_err());
        assert!(render_inline(Format::Pikchr, &"x".repeat(MAX_SOURCE + 1), None).is_err());
        assert!(render_inline(Format::Svgbob, &"+".repeat(257), None).is_err());
        assert!(render_inline(Format::Svgbob, &"x\n".repeat(129), None).is_err());
    }

    #[test]
    fn diagrams_keep_warnings_in_the_raster_result() {
        let rendered =
            render_inline(Format::Lini, "|box#cat| \"cat\"\ncta -> bird\n", None).unwrap();
        assert!(
            rendered
                .warnings
                .iter()
                .any(|item| item.contains("did you mean"))
        );
        let rendered = render_inline(
            Format::Lini,
            "|image| { src: \"https://example.invalid/photo.png\"; width: 40; height: 40 }",
            None,
        )
        .unwrap();
        assert!(
            rendered
                .warnings
                .iter()
                .any(|item| item.contains("images are not displayed"))
        );
    }
}

struct Cached {
    format: Format,
    foreground: Option<[u8; 3]>,
    source: String,
    result: Result<std::sync::Arc<Thumbnail>, String>,
    prepared: std::time::Instant,
}

impl Cached {
    fn fresh(&self) -> bool {
        let ttl = if self.result.is_err() {
            Some(Duration::from_secs(10))
        } else if self.format == Format::Mermaid {
            Some(Duration::from_secs(60))
        } else {
            None
        };
        ttl.is_none_or(|ttl| self.prepared.elapsed() < ttl)
    }
}

#[derive(Debug)]
pub(crate) struct Thumbnail {
    pub math: Option<MathLayout>,
    pub image: little_clip_core::ImageThumbnail,
    pub warnings: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Cache(std::collections::VecDeque<Cached>);
impl Cache {
    pub(crate) fn thumbnail(
        &mut self,
        format: Format,
        source: &str,
    ) -> Result<std::sync::Arc<Thumbnail>, String> {
        self.thumbnail_colored(format, source, None)
    }

    pub(crate) fn thumbnail_colored(
        &mut self,
        format: Format,
        source: &str,
        foreground: Option<[u8; 3]>,
    ) -> Result<std::sync::Arc<Thumbnail>, String> {
        if let Some(index) = self.0.iter().position(|entry| {
            entry.foreground == foreground
                && entry.format == format
                && entry.source == source
                && entry.fresh()
        }) {
            let entry = self.0.remove(index).ok_or("Diagram cache changed")?;
            let result = entry.result.clone();
            self.0.push_back(entry);
            return result;
        }
        validate_source(format, source)?;
        self.0.retain(|entry| {
            entry.format != format || entry.source != source || entry.foreground != foreground
        });
        let result = render_request(format, source, foreground, &AtomicBool::new(false)).and_then(
            |rendered| {
                little_clip_core::ImageThumbnail::generate(&rendered.png, WIDTH, HEIGHT)
                    .map(|image| {
                        std::sync::Arc::new(Thumbnail {
                            image,
                            warnings: rendered.warnings,
                            math: rendered.math,
                        })
                    })
                    .ok_or_else(|| "Invalid diagram raster".into())
            },
        );
        // Many small inline formulas share this cache with large diagrams.
        // Bound both entry count and retained pixels instead of evicting every
        // formula in a dense visible row on each screen refresh.
        let incoming = result
            .as_ref()
            .map_or(0, |preview| preview.image.premultiplied_rgba.len());
        while self.0.len() >= 32
            || self
                .0
                .iter()
                .map(|entry| {
                    entry
                        .result
                        .as_ref()
                        .map_or(0, |preview| preview.image.premultiplied_rgba.len())
                })
                .sum::<usize>()
                + incoming
                > 48 * 1024 * 1024
        {
            if self.0.pop_front().is_none() {
                break;
            }
        }
        self.0.push_back(Cached {
            format,
            foreground,
            source: source.to_owned(),
            result: result.clone(),
            prepared: std::time::Instant::now(),
        });
        result
    }
}
