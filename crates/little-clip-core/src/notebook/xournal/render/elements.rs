use super::*;
use crate::notebook::container as archives;

pub(super) fn draw(
    svg: &mut String,
    node: Node<'_>,
    input: &mut (impl Read + Seek),
    packaged: bool,
    scale: f64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    match node.tag_name().name() {
        "stroke" => stroke(svg, node, cancel),
        "text" => text(svg, node),
        "image" | "teximage" => image(svg, node, input, packaged, scale, cancel),
        "timestamp" | "audio" => Ok(()),
        other => Err(format!("Unsupported Xournal page element: {other}")),
    }
}

fn content(node: Node<'_>) -> String {
    node.children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|node| node.text())
        .collect()
}

struct StrokeStyle<'a> {
    ink: String,
    alpha: f64,
    cap: &'a str,
    dash: String,
    highlighter: bool,
}
impl<'a> StrokeStyle<'a> {
    fn read(node: Node<'a>, width: f64) -> Result<Self, String> {
        let tool = node.attribute("tool").unwrap_or("pen");
        if !matches!(tool, "pen" | "eraser" | "highlighter") {
            return Err("Unsupported Xournal stroke tool".into());
        }
        let (ink, mut alpha) = color(node.attribute("color").unwrap_or("black"))?;
        if tool == "highlighter" && alpha == 1.0 {
            alpha = 0.5;
        }
        let cap = node.attribute("capStyle").unwrap_or("round");
        if !matches!(cap, "round" | "square" | "butt") {
            return Err("Unsupported Xournal stroke cap".into());
        }
        let dash = match node.attribute("style").unwrap_or("") {
            "" | "plain" => String::new(),
            "dash" => format!("stroke-dasharray='{} {}'", width * 5.0, width * 3.0),
            "dot" => format!("stroke-dasharray='{} {}'", width, width * 3.0),
            "dashdot" => format!(
                "stroke-dasharray='{} {} {} {}'",
                width * 5.0,
                width * 3.0,
                width,
                width * 3.0
            ),
            _ => return Err("Unsupported Xournal stroke dash pattern".into()),
        };
        Ok(Self {
            ink,
            alpha,
            cap,
            dash,
            highlighter: tool == "highlighter",
        })
    }
}

fn widths(node: Node<'_>) -> Result<(f64, Vec<f64>), String> {
    let widths = numbers(node.attribute("width").unwrap_or("1"))?;
    let nominal = *widths.first().ok_or("Missing Xournal stroke width")?;
    if nominal <= 0.0 || nominal > 4096.0 {
        return Err("Invalid Xournal stroke width".into());
    }
    let pressure = if let Some(values) = node.attribute("pressures") {
        numbers(values)?
    } else {
        widths[1..].to_vec()
    };
    Ok((nominal, pressure))
}

fn stroke(svg: &mut String, node: Node<'_>, cancel: &AtomicBool) -> Result<(), String> {
    let coords = numbers(&content(node))?;
    if coords.len() < 4 || coords.len() % 2 != 0 {
        return Err("Invalid Xournal stroke coordinates".into());
    }
    let points = coords.as_chunks::<2>().0;
    let (nominal, pressure) = widths(node)?;
    if !pressure.is_empty() && pressure.len() < points.len() - 1 {
        return Err("Xournal stroke has fewer pressures than segments".into());
    }
    let style = StrokeStyle::read(node, nominal)?;
    svg.push_str(&format!(
        "<g opacity='{}'{}>",
        style.alpha,
        if style.highlighter {
            " style='mix-blend-mode:multiply'"
        } else {
            ""
        }
    ));
    let path = points
        .iter()
        .enumerate()
        .map(|(index, [x, y])| format!("{}{x} {y}", if index == 0 { "M" } else { "L" }))
        .collect::<String>();
    filling(svg, node, &path, &style.ink)?;
    if pressure.is_empty() || style.highlighter {
        svg.push_str(&format!("<path d='{path}' stroke='{}' fill='none' stroke-width='{nominal}' stroke-linecap='{}' stroke-linejoin='round' {}/>",style.ink,style.cap,style.dash));
    } else {
        pressure_path(svg, points, &pressure, &style, cancel)?;
    }
    svg.push_str("</g>");
    Ok(())
}

fn filling(svg: &mut String, node: Node<'_>, path: &str, ink: &str) -> Result<(), String> {
    if let Some(fill) = node.attribute("fill") {
        let fill: i32 = fill.parse().map_err(|_| "Invalid Xournal fill opacity")?;
        if !(-1..=255).contains(&fill) {
            return Err("Invalid Xournal fill opacity".into());
        }
        if fill >= 0 {
            svg.push_str(&format!(
                "<path d='{path}Z' fill='{ink}' fill-opacity='{}'/>",
                f64::from(fill) / 255.0
            ));
        }
    }
    Ok(())
}

fn pressure_path(
    svg: &mut String,
    points: &[[f64; 2]],
    pressure: &[f64],
    style: &StrokeStyle<'_>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut offset = 0.0;
    for (index, pair) in points.windows(2).enumerate() {
        if index % 4096 == 0 {
            check_cancel(cancel)?;
            budget(svg)?;
        }
        let width = pressure[index];
        if width <= 0.0 || width > 4096.0 {
            return Err("Invalid Xournal pressure width".into());
        }
        let [a, b] = [pair[0], pair[1]];
        if style.dash.is_empty() {
            tapered(
                svg,
                a,
                b,
                width,
                pressure
                    .get(index + 1)
                    .copied()
                    .filter(|value| *value > 0.0)
                    .unwrap_or(width),
                &style.ink,
            )?;
        } else {
            svg.push_str(&format!("<path d='M{} {}L{} {}' stroke='{}' fill='none' stroke-width='{width}' stroke-linecap='{}' {} stroke-dashoffset='{}'/>",a[0],a[1],b[0],b[1],style.ink,style.cap,style.dash,-offset));
        }
        offset += (b[0] - a[0]).hypot(b[1] - a[1]);
    }
    Ok(())
}

fn tapered(
    svg: &mut String,
    a: [f64; 2],
    b: [f64; 2],
    start: f64,
    end: f64,
    ink: &str,
) -> Result<(), String> {
    if end > 4096.0 {
        return Err("Invalid Xournal pressure width".into());
    }
    let length = (b[0] - a[0]).hypot(b[1] - a[1]);
    let [nx, ny] = if length > 0.0 {
        [(a[1] - b[1]) / length, (b[0] - a[0]) / length]
    } else {
        [0.0, 0.0]
    };
    svg.push_str(&format!(
        "<path fill='{ink}' d='M{} {}L{} {}L{} {}L{} {}Z'/>",
        a[0] + nx * start / 2.0,
        a[1] + ny * start / 2.0,
        b[0] + nx * end / 2.0,
        b[1] + ny * end / 2.0,
        b[0] - nx * end / 2.0,
        b[1] - ny * end / 2.0,
        a[0] - nx * start / 2.0,
        a[1] - ny * start / 2.0
    ));
    for (p, width) in [(a, start), (b, end)] {
        svg.push_str(&format!(
            "<circle cx='{}' cy='{}' r='{}' fill='{ink}'/>",
            p[0],
            p[1],
            width / 2.0
        ));
    }
    Ok(())
}

fn matrix(node: Node<'_>) -> Result<String, String> {
    if let Some(matrix) = node.attribute("matrix") {
        let values = numbers(matrix)?;
        if values.len() != 6 {
            return Err("Invalid Xournal transformation matrix".into());
        }
        Ok(format!(
            "matrix({} {} {} {} {} {})",
            values[0], values[1], values[2], values[3], values[4], values[5]
        ))
    } else {
        Ok(format!(
            "translate({} {})",
            attr(node, "x")?,
            attr(node, "y")?
        ))
    }
}

struct TextFont {
    family: &'static str,
    index: usize,
    bold: bool,
    italic: bool,
}
impl TextFont {
    fn select(name: &str) -> Self {
        let requested = name.to_lowercase();
        let bold = requested.contains("bold");
        let italic = requested.contains("italic") || requested.contains("oblique");
        let (family, index) = if requested.contains("mono") || requested.contains("courier") {
            (
                "Cousine",
                match (bold, italic) {
                    (true, true) => 3,
                    (true, false) => 2,
                    (false, true) => 4,
                    _ => 5,
                },
            )
        } else if requested.contains("times")
            || requested.contains("serif") && !requested.contains("sans")
        {
            (
                "Tinos",
                match (bold, italic) {
                    (true, true) => 7,
                    (true, false) => 6,
                    (false, true) => 8,
                    _ => 9,
                },
            )
        } else {
            ("Arimo", usize::from(!italic))
        };
        Self {
            family,
            index,
            bold,
            italic,
        }
    }
}

fn text(svg: &mut String, node: Node<'_>) -> Result<(), String> {
    let text = content(node);
    if text.len() > 1024 * 1024 {
        return Err("Xournal text exceeds its byte budget".into());
    }
    let size = attr(node, "size")?;
    if !(0.1..=4096.0).contains(&size) {
        return Err("Invalid Xournal font size".into());
    }
    let font = TextFont::select(node.attribute("font").unwrap_or("Sans"));
    let TextFont {
        family,
        index,
        bold,
        italic,
    } = font;
    let metric = ttf_parser::Face::parse(crate::font_assets::GRAPHVIZ_FONT_BYTES[index], 0)
        .map_err(|error| error.to_string())?;
    let units = f64::from(metric.units_per_em());
    let measure = |text: &str| {
        text.chars()
            .map(|c| {
                metric
                    .glyph_index(c)
                    .and_then(|id| metric.glyph_hor_advance(id))
                    .map_or(size * 0.6, |width| f64::from(width) * size / units)
            })
            .sum::<f64>()
    };
    let wrap = node
        .attribute("wrap")
        .map(number)
        .transpose()?
        .filter(|width| *width > 0.0);
    let lines = wrap_lines(&expand_tabs(&text, &measure), wrap, &measure);
    let (ink, alpha) = color(node.attribute("color").unwrap_or("black"))?;
    svg.push_str(&format!("<g transform='{}' fill='{ink}' fill-opacity='{alpha}' font-family='{family}' font-size='{size}' font-weight='{}' font-style='{}'>",matrix(node)?,if bold {"bold"} else {"normal"},if italic {"italic"} else {"normal"}));
    paint_lines(svg, node, &lines, wrap, size, &metric, &measure)?;
    svg.push_str("</g>");
    Ok(())
}

fn paint_lines(
    svg: &mut String,
    node: Node<'_>,
    lines: &[String],
    wrap: Option<f64>,
    size: f64,
    metric: &ttf_parser::Face<'_>,
    measure: &impl Fn(&str) -> f64,
) -> Result<(), String> {
    let units = f64::from(metric.units_per_em());
    let advance =
        f64::from(metric.ascender() - metric.descender() + metric.line_gap()) * size / units;
    for (index, line) in lines.iter().enumerate() {
        if index > 10000 {
            return Err("Xournal text exceeds its line budget".into());
        }
        let width = measure(line);
        let limit = wrap.unwrap_or(width);
        let spaces = line.chars().filter(|c| *c == ' ').count();
        let justify =
            node.attribute("justify") == Some("true") && index + 1 < lines.len() && spaces > 0;
        let aligned_width = if justify { limit } else { width };
        let x = match node.attribute("align").unwrap_or("left") {
            "left" => 0.0,
            "center" => (limit - aligned_width) / 2.0,
            "right" => limit - aligned_width,
            _ => return Err("Unsupported Xournal text alignment".into()),
        };
        let y = f64::from(metric.ascender()) * size / units + index as f64 * advance;
        let extra = if justify {
            (limit - width).max(0.0) / spaces as f64
        } else {
            0.0
        };
        svg.push_str(&format!(
            "<text x='{x}' y='{y}' word-spacing='{extra}' xml:space='preserve'>{}</text>",
            escaped(line)
        ));
    }
    Ok(())
}

fn expand_tabs(text: &str, measure: &impl Fn(&str) -> f64) -> String {
    let space = measure(" ").max(0.01);
    let tab = 8.0 * space;
    let mut result = String::new();
    let mut width = 0.0;
    for c in text.chars() {
        if c == '\t' {
            let target = (width / tab).floor().mul_add(tab, tab);
            let count = ((target - width) / space).round().max(1.0) as usize;
            result.extend(std::iter::repeat_n(' ', count));
            width += count as f64 * space;
        } else {
            result.push(c);
            if c == '\n' {
                width = 0.0;
            } else {
                width += measure(c.encode_utf8(&mut [0; 4]));
            }
        }
    }
    result
}

fn wrap_lines(text: &str, wrap: Option<f64>, measure: &impl Fn(&str) -> f64) -> Vec<String> {
    let Some(wrap) = wrap else {
        return text.split('\n').map(str::to_owned).collect();
    };
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut width = 0.0;
        let mut start = 0;
        for (end, _) in unicode_linebreak::linebreaks(paragraph) {
            let token = &paragraph[start..end];
            start = end;
            let token_width = measure(token);
            if width + measure(token.trim_end()) > wrap && !line.is_empty() {
                lines.push(line.trim_end().to_owned());
                line.clear();
                width = 0.0;
            }
            line.push_str(token);
            width += token_width;
        }
        lines.push(line);
    }
    lines
}

fn image(
    svg: &mut String,
    node: Node<'_>,
    input: &mut (impl Read + Seek),
    packaged: bool,
    scale: f64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let bytes = image_bytes(node, input, packaged, cancel)?;
    let (encoded, width, height) = if bytes.starts_with(b"%PDF-") {
        embedded_pdf(&bytes, node, scale)?
    } else {
        let (encoded, width, height) = raster(&bytes)?;
        (encoded, f64::from(width), f64::from(height))
    };
    if node.attribute("matrix").is_some() {
        svg.push_str(&format!("<image width='{width}' height='{height}' transform='{}' xlink:href='data:image/png;base64,{encoded}'/>",matrix(node)?));
    } else {
        let (x, y, right, bottom) = (
            attr(node, "left")?,
            attr(node, "top")?,
            attr(node, "right")?,
            attr(node, "bottom")?,
        );
        if right <= x || bottom <= y {
            return Err("Invalid Xournal image bounds".into());
        }
        svg.push_str(&format!("<image x='{x}' y='{y}' width='{}' height='{}' preserveAspectRatio='none' xlink:href='data:image/png;base64,{encoded}'/>",right-x,bottom-y));
    }
    Ok(())
}

fn image_bytes(
    node: Node<'_>,
    input: &mut (impl Read + Seek),
    packaged: bool,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let encoded = content(node)
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<String>();
    if encoded.len() > 44 * 1024 * 1024 {
        return Err("Xournal image exceeds its encoded byte budget".into());
    }
    let bytes = if !encoded.is_empty() {
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| error.to_string())?
    } else if packaged {
        let reference = node
            .attribute("filename")
            .or_else(|| {
                node.children()
                    .find(|n| n.has_tag_name("attachment"))
                    .and_then(|n| n.attribute("path").or_else(|| n.attribute("filename")))
            })
            .ok_or("Missing Xournal image attachment")?;
        archives::member(input, reference, 32 * 1024 * 1024, cancel)?
    } else {
        return Err("Missing Xournal image data".into());
    };
    Ok(bytes)
}

fn embedded_pdf(bytes: &[u8], node: Node<'_>, scale: f64) -> Result<(String, f64, f64), String> {
    let pdf = background::Pdf::new(bytes)?;
    let (width, height) = pdf.dimensions()?;
    let transform = if let Some(matrix) = node.attribute("matrix") {
        let matrix = numbers(matrix)?;
        if matrix.len() != 6 {
            return Err("Invalid Xournal image matrix".into());
        }
        matrix[..4]
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt()
    } else {
        ((attr(node, "right")? - attr(node, "left")?) / width)
            .max((attr(node, "bottom")? - attr(node, "top")?) / height)
    };
    let edge = (width.max(height) * transform * scale)
        .ceil()
        .clamp(16.0, 4096.0) as u32;
    let (encoded, _, _) = raster(&pdf.page(1, edge)?)?;
    Ok((encoded, width, height))
}
