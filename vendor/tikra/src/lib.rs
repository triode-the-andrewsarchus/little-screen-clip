//! tikra: TikZ figures to SVG and PNG, with no TeX install. Math labels are drawn by mathra.
//!
//! ```
//! let r = tikra::convert(r"\begin{tikzpicture} \draw[->] (0,0) -- (1,1) node[right] {$x$}; \end{tikzpicture}").unwrap();
//! assert!(r.svg.starts_with("<svg"));
//! ```

// The engine's parts: public for tikra's own tests, not an API (they change with any release).
// What is kept stable is `convert`, `png` and `extension`.
#[doc(hidden)]
pub mod canvas;
#[doc(hidden)]
pub mod color;
#[doc(hidden)]
pub mod describe;
#[doc(hidden)]
pub mod driver;
#[doc(hidden)]
pub mod engine;
#[doc(hidden)]
pub mod expr;
pub mod extension;
#[doc(hidden)]
pub mod keys;
#[doc(hidden)]
pub mod label;
#[cfg(feature = "motion")]
pub mod motion;
#[doc(hidden)]
pub mod pen;
pub mod scene;
mod slides;
#[doc(hidden)]
pub mod tex;
mod times;

use std::fmt;

/// Centimeters per TeX point.
pub const PT: f64 = 2.54 / 72.27;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Expression(String),
    Figure(String),
    Png(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Expression(m) => write!(f, "expression: {m}"),
            Error::Figure(m) => write!(f, "figure: {m}"),
            Error::Png(m) => write!(f, "png: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// The language the figure is told in (its `alt` and `description`): mathra's 16 languages. The
/// drawing is the same in all of them.
pub use mathra::locale::Locale;

/// How to convert.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// The language of the figure in words. English by default.
    pub locale: Locale,
    /// The construction animation: the SVG draws itself, path by path, in the order TikZ drew it
    /// (CSS in the SVG; browsers play it, PNG ignores it). Off by default.
    pub animate: bool,
    /// Each element of the SVG numbered by its step (`data-step`, one per path, label or image, in
    /// the order TikZ drew them) and the count on the `<svg>` (`data-steps`), with no CSS: for a page
    /// that walks through the figure itself. The animation numbers them too. Off by default.
    pub steps: bool,
    /// Each element numbered by its step, and with the place in the code that drew it
    /// (`data-src="line:column-line:column"`, from 1): what a macro or a `\\foreach` draws is at
    /// the line that called it, an `\\addplot` at its own. Off by default.
    pub source: bool,
    /// Each thing the figure's code draws is a part a reader can reach: a group with its words
    /// (`aria-label`, in `locale`) that takes the keyboard's focus, inside an `<svg>` a screen
    /// reader can enter. The numbers and places of [`Options::source`] come with it. Off by default.
    pub parts: bool,
    /// The animations library played in a browser, with SMIL, `begin on=click` included, instead of
    /// the still picture TikZ would give. A PNG stays still. Off by default.
    pub play: bool,
    /// Each bar and mark of a plot is an element of its own, with its values (`data-x`, `data-y`) and
    /// a tip for the mouse and the keyboard; with the animation, the bars grow one by one. Off by
    /// default.
    pub points: bool,
}

impl Options {
    pub fn locale(locale: Locale) -> Options {
        Options { locale, animate: false, steps: false, source: false, parts: false, play: false, points: false }
    }

    /// These options, with the construction animation.
    pub fn animated(self) -> Options {
        Options { animate: true, ..self }
    }

    /// These options, with each element numbered by its step.
    pub fn steps(self) -> Options {
        Options { steps: true, ..self }
    }

    /// These options, with each element numbered by its step and tied to the code that drew it.
    pub fn source(self) -> Options {
        Options { source: true, ..self }
    }

    /// These options, with each point of a plot an element of its own.
    pub fn points(self) -> Options {
        Options { points: true, ..self }
    }

    /// These options, with the animations library played.
    pub fn play(self) -> Options {
        Options { play: true, ..self }
    }

    /// These options, with the figure split in parts a reader can walk through.
    pub fn parts(self) -> Options {
        Options { parts: true, ..self }
    }
}

#[derive(Debug, Clone)]
pub struct Output {
    pub svg: String,
    /// The figure in a sentence, for readers who cannot see it: the SVG's `<title>`, an image's `alt`.
    pub alt: String,
    /// The figure in words: what it shows (axes, data, nodes and their connections), the SVG's `<desc>`.
    ///
    /// Written by tikra from what it drew, it is not perfect: it says what is in the drawing, not
    /// what the figure means, and says little about free drawings. Review it; the author's own text
    /// (`alt={…}` on the picture) is told instead when there is one.
    pub description: String,
    /// What was ignored or approximated (unsupported commands, labels with mathra warnings).
    pub warnings: Vec<String>,
    /// The nodes and coordinates the author named (`\coordinate (top) at (0,2);`), where they are
    /// in the SVG's own coordinates (the viewBox's units, y down), once per name, where it was
    /// last placed: a page can hang handles, hints or answers on them.
    pub points: Vec<NamedPoint>,
    /// Where the picture's origin, (0,0), is in the SVG. The drawing is moved so that its box starts
    /// at the SVG's corner, so the same point lands elsewhere when the figure grows to the left or
    /// up; less `origin`, it does not: the way to follow a point from one drawing to the next.
    pub origin: (f64, f64),
    /// What each statement drew, in words, in the order of the code, with the source map or the
    /// parts: a page's caption for a step, say.
    pub parts: Vec<Part>,
}

/// A statement of the code and what it drew, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Its place in the code, as in `data-src`.
    pub source: String,
    /// A short name for it: `aria-label` of the SVG's parts.
    pub text: String,
    /// A sentence for a caption of the step. Empty when there is nothing to say but that a shape
    /// was drawn: better no caption than a hollow one.
    pub sentence: String,
}

/// A node or coordinate with a name, and where it is in the SVG (see [`Output::points`]).
#[derive(Debug, Clone, PartialEq)]
pub struct NamedPoint {
    pub name: String,
    pub x: f64,
    pub y: f64,
}

/// Accepts a full `standalone` document, a bare `tikzpicture`, an inline `\tikz` or an `axis`.
pub fn convert(code: &str) -> Result<Output, Error> {
    convert_with(code, &Options::default())
}

/// [`convert`], with the figure in words in `options.locale`.
pub fn convert_with(code: &str, options: &Options) -> Result<Output, Error> {
    convert_with_libraries(code, options, &[])
}

/// [`convert_with`], with libraries from outside tikra defined next to TikZ's own (see [`extension`]).
pub fn convert_with_libraries(code: &str, options: &Options, libraries: &[&'static extension::Library]) -> Result<Output, Error> {
    let (first, count) = convert_slide(code, options, libraries, 1)?;
    // overlays: the figure as its last slide leaves it
    Ok(if count > 1 { convert_slide(code, options, libraries, count)?.0 } else { first })
}

/// Each slide of a figure with Beamer's overlays (`\\pause`, `\\only<2->{…}`, `\\uncover<…>{…}`,
/// `\\draw<2->`…), in order. A figure without them has one. [`click_through`] makes one SVG of them.
pub fn slides(code: &str, options: &Options) -> Result<Vec<Output>, Error> {
    let mut out = Vec::new();
    let mut count = 1;
    while out.len() < count {
        let (o, more) = convert_slide(code, options, &[], out.len() + 1)?;
        count = count.max(more);
        out.push(o);
    }
    Ok(out)
}

/// The slides of [`slides`] as one SVG that advances on a click, and comes back to the first after
/// the last. It takes a browser, opening it or inline in a page: an `<img>` takes no clicks.
pub fn click_through(slides: &[Output]) -> Option<String> {
    slides::click_through(slides)
}

/// A slide of the figure, and how many slides its code has shown.
fn convert_slide(code: &str, options: &Options, libraries: &[&'static extension::Library], slide: usize) -> Result<(Output, usize), Error> {
    label::set_kerning(mathra::layout::kerning::TextKerning::Ot1);
    let mut early = Vec::new();
    // TikZ escaped twice (\\draw, \\coordinate): the same repair mathra applies to equations
    let repaired = mathra::input::latex::undo_doubled_backslashes(code);
    if repaired.is_some() {
        early.push("doubled backslashes (code escaped twice): undid one level".to_string());
    }
    let mut base = repaired.unwrap_or_else(|| code.to_string());
    if let Some(r) = repair_markdown(&base) {
        early.push("TikZ escaped for Markdown (```, \\\\draw, \\[ \\]): undone".to_string());
        base = r;
    }
    let text = strip_comments(&base);
    let mut wrapped = false;
    let input = if text.contains("\\begin{tikzpicture}") || text.contains("\\begin{pgfpicture}") || text.contains("\\pgfuseshading") || tikz_command(&text).is_some() {
        base
    } else if text.contains("\\begin{axis}") || ["\\draw", "\\fill", "\\node", "\\path", "\\filldraw", "\\coordinate"].iter().any(|c| text.contains(c)) {
        // an axis or loose statements, with no tikzpicture around them
        wrapped = true;
        format!("\\begin{{tikzpicture}}\\relax\n{base}\n\\end{{tikzpicture}}")
    } else {
        return Err(Error::Figure("no tikzpicture, \\tikz or axis found".into()));
    };
    let wrapped = usize::from(wrapped);
    let (mut canvas, warnings, facts) =
        engine::run_document_mapped(&input, libraries, engine::Run { map: options.source || options.parts, slide, play: options.play, points: options.points })?;
    for s in &mut canvas.sources {
        s.first.0 = s.first.0.saturating_sub(wrapped).max(1);
        s.last.0 = s.last.0.saturating_sub(wrapped).max(1);
    }
    let words = describe::describe(&facts, options.locale);
    let parts: Vec<Part> = (0..canvas.sources.len())
        .map(|id| (id, describe::describe_part(&facts, id, options.locale)))
        .filter(|(_, (text, _))| !text.is_empty())
        .map(|(id, (text, sentence))| {
            let s = canvas.sources[id];
            Part { source: format!("{}:{}-{}:{}", s.first.0, s.first.1, s.last.0, s.last.1), text, sentence }
        })
        .collect();
    if options.parts {
        canvas.parts = (0..canvas.sources.len()).map(|id| describe::describe_part(&facts, id, options.locale).0).collect();
    }
    let svg = driver::svg_with(&canvas, code, &words, options.steps || options.source || options.parts, options.animate);
    let mut all = early;
    all.extend(warnings);
    let mut points: Vec<NamedPoint> = Vec::new();
    for (name, p) in &canvas.named {
        let at = NamedPoint { name: name.clone(), x: p.x * driver::BP, y: -p.y * driver::BP };
        match points.iter_mut().find(|q| q.name == *name) {
            Some(q) => *q = at,
            None => points.push(at),
        }
    }
    let origin = canvas.origin.map_or((0.0, 0.0), |d| (d.x * driver::BP, -d.y * driver::BP));
    let count = canvas.slides;
    Ok((Output { svg, alt: words.alt, description: words.text, warnings: all, points, origin, parts }, count))
}

/// TikZ that went through a Markdown generator: code fences, `\\draw` and `\[red\]`.
fn repair_markdown(t: &str) -> Option<String> {
    const COMMANDS: &[&str] = &["draw", "fill", "filldraw", "path", "node", "coordinate", "foreach", "begin", "end", "def", "clip", "tikzset", "shade", "pgfmathsetmacro"];
    let mut changed = false;
    let mut s: String = t
        .lines()
        .filter(|l| {
            let fence = l.trim_start().starts_with("```");
            changed |= fence;
            !fence
        })
        .collect::<Vec<_>>()
        .join("\n");
    for c in COMMANDS {
        let doubled = format!("\\\\{c}");
        // (not `\\\end`: a line break before `\end`)
        let at: Vec<usize> = s.match_indices(&doubled).map(|(i, _)| i).filter(|&i| !s[..i].ends_with('\\')).collect();
        for &i in at.iter().rev() {
            s.replace_range(i..i + 1, "");
            changed = true;
        }
    }
    // `\[` is display math in TeX: brackets are escaped only where an option list opens, right
    // after a command of TikZ or its environment (`\draw\[`, `\begin{tikzpicture}\[`), or where
    // `\[` holds such a command (`\[\draw`; not `\[\begin{array}`, display math)
    let drawing = COMMANDS.iter().filter(|c| !matches!(**c, "begin" | "end" | "def")).map(|c| format!("\\{c}"));
    let opening: Vec<String> = drawing.clone().chain(["\\begin{tikzpicture}".to_string(), "\\begin{scope}".to_string()]).collect();
    let inside: Vec<String> = drawing.collect();
    let escaped = s.match_indices("\\[").any(|(i, _)| {
        let after = s[i + 2..].trim_start();
        opening.iter().any(|c| s[..i].ends_with(c.as_str()))
            || inside.iter().any(|c| after.strip_prefix(c.as_str()).is_some_and(|r| !r.starts_with(|x: char| x.is_ascii_alphabetic())))
    });
    if escaped {
        // a Markdown \[ inside \draw\[…\]; \\[2pt] (a line break with spacing) is left alone
        let c: Vec<char> = s.chars().collect();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < c.len() {
            if c[i] == '\\' && matches!(c.get(i + 1), Some('[') | Some(']')) && (i == 0 || c[i - 1] != '\\') {
                out.push(c[i + 1]);
                i += 2;
                changed = true;
                continue;
            }
            out.push(c[i]);
            i += 1;
        }
        s = out;
    }
    changed.then_some(s)
}

/// The text with TeX comments (`%` to the end of the line, not `\%`) removed.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let c: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < c.len() {
            if c[i] == '\\' && i + 1 < c.len() {
                out.push(c[i]);
                out.push(c[i + 1]);
                i += 2;
                continue;
            }
            if c[i] == '%' {
                break;
            }
            out.push(c[i]);
            i += 1;
        }
        out.push('\n');
    }
    out
}

/// Position of `\\tikz` as a command (not `\\tikzset` or `\\tikzstyle`).
fn tikz_command(text: &str) -> Option<usize> {
    text.match_indices("\\tikz").map(|(i, _)| i).find(|&i| !text[i + 5..].starts_with(|c: char| c.is_ascii_alphabetic()))
}

/// PNG pixel cap (100 megapixels, ~400 MB of memory).
pub const MAX_PIXELS: u64 = 100_000_000;

/// Rasterizes the SVG to PNG. `dpi` 300 is the print default; `background` paints it white.
pub fn png(svg: &str, dpi: f32, background: bool) -> Result<Vec<u8>, Error> {
    use resvg::{tiny_skia, usvg};
    if !(dpi > 0.0 && dpi.is_finite()) {
        return Err(Error::Png(format!("invalid dpi: {dpi}")));
    }
    // a PNG is still: the construction animation's rules would leave the strokes undrawn
    let still = match (svg.find("<style>@keyframes tr-d"), svg.find("</style>")) {
        (Some(i), Some(j)) if i < j => format!("{}{}", &svg[..i], &svg[j + "</style>".len()..]),
        _ => svg.to_string(),
    };
    let tree = usvg::Tree::from_str(&still, &usvg::Options::default()).map_err(png_error)?;
    // usvg converts the `pt` of the width attribute at 96 px per inch: the scale is relative to that
    let scale = dpi / 96.0;
    // whole pixels, rounded as a PDF rasterizer rounds the page, and the drawing on them from the
    // bottom left corner, where PDF's origin is: lines fall on the pixels where TeX's do
    let (w, h) = (f64::from(tree.size().width() * scale), f64::from(tree.size().height() * scale));
    if w * h > MAX_PIXELS as f64 || !(w * h).is_finite() {
        return Err(Error::Png(format!("image too large: {w:.0} × {h:.0} px at {dpi} dpi (cap of {MAX_PIXELS} pixels)")));
    }
    let (width, height) = (w.round() as u32, h.round() as u32);
    if width == 0 || height == 0 {
        return Err(Error::Png("invalid size".into()));
    }
    let mut image = tiny_skia::Pixmap::new(width, height).ok_or(Error::Png("image too large".into()))?;
    if background {
        image.fill(tiny_skia::Color::WHITE);
    }
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, 0.0, (f64::from(height) - h) as f32), &mut image.as_mut());
    image.encode_png().map_err(png_error)
}

/// An SVG with no animation as RGBA pixels on white, stretched to `width` × `height`.
#[cfg(feature = "motion")]
pub(crate) fn raster(svg: &str, width: u32, height: u32) -> Result<Vec<u8>, Error> {
    use resvg::{tiny_skia, usvg};
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Error::Png(format!("image too large: {width} × {height} px (cap of {MAX_PIXELS} pixels)")));
    }
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(png_error)?;
    let mut image = tiny_skia::Pixmap::new(width, height).ok_or(Error::Png("image too large".into()))?;
    image.fill(tiny_skia::Color::WHITE);
    let (sx, sy) = (width as f32 / tree.size().width(), height as f32 / tree.size().height());
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut image.as_mut());
    Ok(image.take())
}

/// Errors from the rasterization libraries (invalid SVG, PNG encoding failure).
fn png_error(e: impl fmt::Display) -> Error {
    Error::Png(e.to_string())
}
