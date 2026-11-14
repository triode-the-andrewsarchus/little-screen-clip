use super::*;
use crate::notebook::container as archives;
use std::{fs::File, io::Write, path::PathBuf};

pub(super) struct Guard {
    file: File,
    source: crate::file_source::FileSource,
}
impl Guard {
    pub fn validate(&self) -> Result<(), String> {
        self.source.check_file(&self.file)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Page background resolution retains container, original directory, dimensions and cancellation together"
)]
pub(super) fn draw(
    svg: &mut String,
    page: Node<'_>,
    pages: &[Node<'_>],
    input: &mut (impl Read + Seek),
    source: &Path,
    packaged: bool,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<Option<Guard>, String> {
    let width = attr(page, "width")?;
    let height = attr(page, "height")?;
    let Some(bg) = resolve_background(page, pages)? else {
        svg.push_str(&format!(
            "<rect width='{width}' height='{height}' fill='white'/>"
        ));
        return Ok(None);
    };
    let kind = bg.attribute("type").unwrap_or("solid");
    if kind == "solid" {
        solid(svg, bg, width, height)?;
        return Ok(None);
    }
    if !matches!(kind, "pdf" | "pixmap") {
        return Err("Unsupported Xournal background type".into());
    }
    let (bytes, guard) = load_reference(bg, pages, input, source, packaged, cancel)?;
    let bytes = if kind == "pdf" {
        pdf_page(&bytes, bg.attribute("pageno").unwrap_or("1"), edge)?
    } else {
        bytes
    };
    let (encoded, _, _) = raster(&bytes)?;
    svg.push_str(&format!("<image width='{width}' height='{height}' preserveAspectRatio='none' xlink:href='data:image/png;base64,{encoded}'/>"));
    Ok(guard)
}

fn resolve_background<'a>(page: Node<'a>, pages: &[Node<'a>]) -> Result<Option<Node<'a>>, String> {
    let Some(mut bg) = page.children().find(|node| node.has_tag_name("background")) else {
        return Ok(None);
    };
    let mut index = pages
        .iter()
        .position(|candidate| *candidate == page)
        .ok_or("Missing Xournal page")?;
    while bg.attribute("domain") == Some("clone") && bg.attribute("type") == Some("pixmap") {
        let previous = integer(
            bg.attribute("filename")
                .ok_or("Missing cloned background page")?,
        )?;
        if previous >= index {
            return Err("Invalid or cyclic Xournal background clone".into());
        }
        index = previous;
        bg = pages[index]
            .children()
            .find(|node| node.has_tag_name("background"))
            .ok_or("Missing cloned background")?;
    }
    Ok(Some(bg))
}

fn load_reference(
    bg: Node<'_>,
    pages: &[Node<'_>],
    input: &mut (impl Read + Seek),
    source: &Path,
    packaged: bool,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, Option<Guard>), String> {
    let reference = if bg.attribute("type") == Some("pdf") {
        pages
            .iter()
            .flat_map(|page| page.children())
            .find(|node| {
                node.has_tag_name("background")
                    && node.attribute("type") == Some("pdf")
                    && node.attribute("filename").is_some()
            })
            .ok_or("Missing Xournal PDF background filename")?
    } else {
        bg
    };
    let name = reference
        .attribute("filename")
        .ok_or("Missing Xournal background filename")?;
    let attached = reference.attribute("domain") == Some("attach");
    if packaged && attached {
        return archives::member(input, name, 128 * 1024 * 1024, cancel).map(|bytes| (bytes, None));
    }
    let path = companion(source, name, attached)?;
    let (mut file, identity) = crate::file_source::ReadCheck::default()
        .open(&path, None, cancel)
        .map_err(|error| format!("Xournal background unavailable: {error}"))?;
    let bytes = archives::bounded(&mut file, 128 * 1024 * 1024, cancel)?;
    identity.check_file(&file)?;
    Ok((
        bytes,
        Some(Guard {
            file,
            source: identity,
        }),
    ))
}

fn pdf_page(bytes: &[u8], page: &str, edge: u32) -> Result<Vec<u8>, String> {
    let page = integer(page)?;
    if page == 0 {
        return Err("Invalid Xournal PDF page number".into());
    }
    Pdf::new(bytes)?.page(page, edge)
}

pub(super) struct Pdf {
    file: tempfile::NamedTempFile,
    source: crate::file_source::FileSource,
}
impl Pdf {
    pub fn new(bytes: &[u8]) -> Result<Self, String> {
        if !bytes.starts_with(b"%PDF-") {
            return Err("Xournal background is not a PDF".into());
        }
        let mut file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        let source = crate::file_source::FileSource::capture_file(file.as_file())?;
        Ok(Self { file, source })
    }
    pub fn page(&self, page: usize, edge: u32) -> Result<Vec<u8>, String> {
        let result = crate::pdf::render_pdf_page_thumbnail(self.file.path(), page, edge)?.0;
        self.source.check_file(self.file.as_file())?;
        Ok(result)
    }
    pub fn dimensions(&self) -> Result<(f64, f64), String> {
        let info = crate::pdf::get_pdf_info(self.file.path())?;
        self.source.check_file(self.file.as_file())?;
        let size = info
            .page_size
            .ok_or("Embedded Xournal PDF has no page size")?;
        let fields = size.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 || fields[1] != "x" {
            return Err("Invalid embedded PDF page size".into());
        }
        let (width, height) = (number(fields[0])?, number(fields[2])?);
        if width <= 0.0 || height <= 0.0 {
            return Err("Invalid embedded PDF dimensions".into());
        }
        Ok((width, height))
    }
}

fn integer(value: &str) -> Result<usize, String> {
    // Older packaged upstream fixtures have a literal C integer suffix. Accept
    // that specific spelling, not arbitrary trailing text or signed values.
    value
        .strip_suffix("ll")
        .unwrap_or(value)
        .parse()
        .map_err(|_| "Invalid Xournal page number".into())
}

fn companion(source: &Path, name: &str, attached: bool) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains("://") || name.starts_with("file:") {
        return Err("External Xournal resource URLs are not supported".into());
    }
    if attached {
        if Path::new(name)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("Invalid Xournal attachment filename".into());
        }
        let mut path = source.as_os_str().to_os_string();
        path.push(".");
        path.push(name);
        Ok(PathBuf::from(path))
    } else {
        let path = Path::new(name);
        Ok(if path.is_absolute() {
            path.into()
        } else {
            source.parent().unwrap_or(Path::new(".")).join(path)
        })
    }
}

struct Paper<'a> {
    width: f64,
    height: f64,
    style: &'a str,
    fill: String,
    opacity: f64,
    ink: String,
    line: f64,
    spacing: f64,
    dark: bool,
    settings: std::collections::BTreeMap<&'a str, &'a str>,
}
impl<'a> Paper<'a> {
    fn new(node: Node<'a>, width: f64, height: f64) -> Result<Self, String> {
        let (fill, opacity) = color(node.attribute("color").unwrap_or("white"))?;
        let rgb = u32::from_str_radix(&fill[1..], 16).map_err(|error| error.to_string())?;
        let dark = ((rgb >> 16) & 255) + ((rgb >> 8) & 255) + (rgb & 255) < 384;
        let style = node.attribute("style").unwrap_or("plain");
        let ruled = matches!(style, "ruled" | "lined");
        let settings = node
            .attribute("config")
            .unwrap_or("")
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .collect();
        let mut paper = Self {
            width,
            height,
            style,
            fill,
            opacity,
            ink: String::new(),
            line: 0.5,
            spacing: 14.17,
            dark,
            settings,
        };
        paper.ink = paper.color(
            if dark { "af1" } else { "f1" },
            match (dark, ruled) {
                (true, _) => "#2f4f4f",
                (false, true) => "#1e90ff",
                _ if style == "staves" => "#000000",
                _ => "#c0c0c0",
            },
        )?;
        paper.line = paper.value(
            "lw",
            if matches!(style, "dotted" | "isodotted") {
                1.5
            } else if style == "isograph" {
                1.0
            } else {
                0.5
            },
        )?;
        paper.spacing = paper.value("r1", if ruled { 24.0 } else { 14.17 })?;
        if !(1.0..=16384.0).contains(&paper.spacing) || !(0.0..=64.0).contains(&paper.line) {
            return Err("Invalid Xournal ruling spacing/width".into());
        }
        Ok(paper)
    }
    fn value(&self, key: &str, default: f64) -> Result<f64, String> {
        self.settings
            .get(key)
            .map_or(Ok(default), |value| number(value))
    }
    fn color(&self, key: &str, default: &str) -> Result<String, String> {
        self.settings.get(key).map_or(Ok(default.into()), |value| {
            color(&format!("#{value}")).map(|value| value.0)
        })
    }
}

fn solid(svg: &mut String, node: Node<'_>, width: f64, height: f64) -> Result<(), String> {
    let p = Paper::new(node, width, height)?;
    svg.push_str(&format!(
        "<rect width='{width}' height='{height}' fill='{}' fill-opacity='{}'/>",
        p.fill, p.opacity
    ));
    if p.style == "plain" {
        return Ok(());
    }
    svg.push_str(&format!(
        "<g stroke='{}' stroke-width='{}' fill='none'>",
        p.ink, p.line
    ));
    match p.style {
        "lined" | "ruled" => ruled(svg, &p)?,
        "graph" | "dotted" => grid(
            svg,
            p.style,
            width,
            height,
            p.spacing,
            p.value("m1", 0.0)?,
            p.settings.get("rm") == Some(&"1"),
        )?,
        "isograph" | "isodotted" => isometric(svg, &p)?,
        "staves" => staves(svg, &p),
        other => return Err(format!("Unsupported Xournal paper style: {other}")),
    }
    svg.push_str("</g>");
    budget(svg)
}

fn ruled(svg: &mut String, p: &Paper<'_>) -> Result<(), String> {
    let mut y = 80.0;
    while y <= p.height - 60.0 {
        svg.push_str(&format!("<path d='M0 {y}H{}'/>", p.width));
        y += p.spacing;
    }
    if p.style == "lined" {
        let margin = p.value("m1", 72.0)?;
        let x = if margin < 0.0 {
            p.width + margin
        } else {
            margin
        };
        let ink = p.color(
            if p.dark { "af2" } else { "f2" },
            if p.dark { "#191970" } else { "#ff1493" },
        )?;
        svg.push_str(&format!("<path stroke='{ink}' d='M{x} 0V{}'/>", p.height));
    }
    Ok(())
}

fn isometric(svg: &mut String, p: &Paper<'_>) -> Result<(), String> {
    if p.width * p.height / (p.spacing * p.spacing) > 200_000.0 {
        return Err("Xournal ruling exceeds the geometry budget".into());
    }
    let dx = p.spacing * 3.0_f64.sqrt() / 2.0;
    let dy = p.spacing / 2.0;
    let cols = ((p.width - 2.0 * p.spacing) / dx).floor().max(0.0) as usize;
    let rows = ((p.height - 2.0 * p.spacing) / dy).floor().max(0.0) as usize;
    let x0 = (p.width - cols as f64 * dx) / 2.0;
    let y0 = (p.height - rows as f64 * dy) / 2.0;
    if p.style == "isograph" {
        svg.push_str(&format!(
            "<path d='M{x0} {y0}h{}M{x0} {}h{}'/>",
            cols as f64 * dx,
            y0 + rows as f64 * dy,
            cols as f64 * dx
        ));
    }
    for col in 0..=cols {
        if p.style == "isograph" {
            let x = x0 + col as f64 * dx;
            svg.push_str(&format!("<path d='M{x} {y0}v{}'/>", rows as f64 * dy));
        }
        for row in 0..=rows {
            if (col + row) % 2 == 0 {
                continue;
            }
            let x = x0 + col as f64 * dx;
            let y = y0 + row as f64 * dy;
            if p.style == "isodotted" {
                svg.push_str(&format!(
                    "<circle cx='{x}' cy='{y}' r='{}' fill='{}' stroke='none'/>",
                    p.line / 2.0,
                    p.ink
                ));
            } else {
                triangle_edges(svg, (x, y), (dx, dy), (col, row), (cols, rows));
            }
        }
    }
    Ok(())
}

fn triangle_edges(
    svg: &mut String,
    (x, y): (f64, f64),
    (dx, dy): (f64, f64),
    (col, row): (usize, usize),
    (cols, rows): (usize, usize),
) {
    if col < cols && row < rows {
        svg.push_str(&format!("<path d='M{x} {y}l{dx} {dy}'/>"));
    }
    if col < cols && row > 0 {
        svg.push_str(&format!("<path d='M{x} {y}l{dx} {}'/>", -dy));
    }
}

fn staves(svg: &mut String, p: &Paper<'_>) {
    // Match the document's historic staff geometry, including its extra line-width gap.
    let mut y = 80.0;
    while y <= p.height - 60.0 {
        for line in 0..5 {
            let y = y + f64::from(line) * 5.0;
            svg.push_str(&format!(
                "<path stroke-linecap='square' d='M50 {y}H{}'/>",
                p.width - 50.0
            ));
        }
        svg.push_str(&format!("<path d='M50 {y}v20M{} {y}v20'/>", p.width - 50.0));
        y += 60.0 + 5.0 * p.line;
    }
}

fn grid(
    svg: &mut String,
    style: &str,
    width: f64,
    height: f64,
    step: f64,
    margin: f64,
    round: bool,
) -> Result<(), String> {
    if width * height / (step * step) > 200_000.0 {
        return Err("Xournal grid exceeds the geometry budget".into());
    }
    let (mut x0, mut y0, mut x1, mut y1) = (
        margin.max(0.0),
        margin.max(0.0),
        width - margin.max(0.0),
        height - margin.max(0.0),
    );
    if round {
        x0 = (x0 / step).ceil() * step;
        y0 = (y0 / step).ceil() * step;
        x1 = (x1 / step).floor() * step;
        y1 = (y1 / step).floor() * step;
    }
    let mut x = (x0 / step).ceil().max(1.0) * step;
    while x <= x1 {
        if style == "dotted" {
            let mut y = (y0 / step).ceil().max(1.0) * step;
            while y <= y1 {
                svg.push_str(&format!("<path stroke-linecap='round' d='M{x} {y}l0 0'/>"));
                y += step;
            }
        } else {
            svg.push_str(&format!("<path d='M{x} {y0}V{y1}'/>"));
        }
        x += step;
    }
    if style == "graph" {
        let mut y = (y0 / step).ceil().max(1.0) * step;
        while y <= y1 {
            svg.push_str(&format!("<path d='M{x0} {y}H{x1}'/>"));
            y += step;
        }
    }
    Ok(())
}
