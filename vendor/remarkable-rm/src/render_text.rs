//! Deterministic Noto Sans text rasterization; no dependency on host fonts.
use crate::{
    document::Page,
    render::{RenderError, RenderOptions},
    scene::text::ParagraphStyle,
};
use std::{
    fmt::Write,
    sync::{Arc, OnceLock},
};
use tiny_skia::{Pixmap, Transform};

fn font_database() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            for bytes in [
                include_bytes!("../fonts/NotoSans-Regular.ttf").as_slice(),
                include_bytes!("../fonts/NotoSans-Bold.ttf").as_slice(),
                include_bytes!("../fonts/NotoSans-Italic.ttf").as_slice(),
                include_bytes!("../fonts/NotoSans-BoldItalic.ttf").as_slice(),
            ] {
                db.load_font_data(bytes.to_vec());
            }
            db.set_sans_serif_family("Noto Sans");
            Arc::new(db)
        })
        .clone()
}

fn xml(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn draw_text(
    page: &Page,
    pixmap: &mut Pixmap,
    opts: RenderOptions,
) -> Result<(), RenderError> {
    let Some(tree) = prepare(page)? else {
        return Ok(());
    };
    let scale = opts.width as f32 / tree.size().width();
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn prepare(page: &Page) -> Result<Option<resvg::usvg::Tree>, RenderError> {
    let Some(text) = &page.text else {
        return Ok(None);
    };
    let layout = text
        .layout()
        .map_err(|e| RenderError::Text(e.to_string()))?;
    let source = RenderOptions::for_page(page);
    let half_width = source.width as f32 / 2.0;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"-{} 0 {} {}\">",
        source.width, source.height, half_width, source.width, source.height
    );
    let mut number = 0;
    for paragraph in layout.paragraphs {
        let style = paragraph.style;
        let prefix = match style {
            ParagraphStyle::Numbered => {
                number += 1;
                format!("{number}. ")
            }
            ParagraphStyle::Bullet | ParagraphStyle::Bullet2 => {
                number = 0;
                "• ".to_owned()
            }
            _ => {
                number = 0;
                String::new()
            }
        };
        let checkbox = matches!(
            style,
            ParagraphStyle::CheckboxChecked | ParagraphStyle::CheckboxUnchecked
        );
        let x = text.pos_x;
        let y = paragraph.baseline;
        if checkbox {
            write!(svg, "<rect x=\"{x}\" y=\"{}\" width=\"24\" height=\"24\" fill=\"none\" stroke=\"black\" stroke-width=\"2\"/>",y-24.0).unwrap();
            if style == ParagraphStyle::CheckboxChecked {
                write!(svg,"<path d=\"M {} {} l 6 6 l 10 -14\" stroke=\"black\" stroke-width=\"2\" fill=\"none\"/>",x+3.0,y-14.0).unwrap();
            }
        }
        let x = x + if checkbox { 36.0 } else { 0.0 };
        let size = if style == ParagraphStyle::Heading {
            48
        } else {
            24
        };
        let weight = if style == ParagraphStyle::Bold {
            "bold"
        } else {
            "normal"
        };
        let decor = if style == ParagraphStyle::CheckboxChecked {
            "line-through"
        } else {
            "none"
        };
        write!(svg,"<text x=\"{x}\" y=\"{y}\" font-family=\"Noto Sans\" font-size=\"{size}\" font-weight=\"{weight}\" text-decoration=\"{decor}\" xml:space=\"preserve\">{}",xml(&prefix)).unwrap();
        for span in paragraph.spans {
            let weight = if span.bold { "bold" } else { weight };
            let italic = if span.italic { "italic" } else { "normal" };
            write!(
                svg,
                "<tspan font-weight=\"{weight}\" font-style=\"{italic}\">{}</tspan>",
                xml(&span.text)
            )
            .unwrap();
        }
        svg.push_str("</text>");
    }
    svg.push_str("</svg>");
    let options = resvg::usvg::Options {
        fontdb: font_database(),
        ..Default::default()
    };
    resvg::usvg::Tree::from_str(&svg, &options)
        .map(Some)
        .map_err(|e| RenderError::Text(e.to_string()))
}
