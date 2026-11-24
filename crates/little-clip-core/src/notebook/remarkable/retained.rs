//! Retain the existing pen model's paint geometry in the document SVG compositor.
use crate::notebook::{check_cancel, vector};
use remarkable_rm::{
    BackgroundImage, CrdtId, Layer, Line, Page, Pen, Point, RenderOptions,
    render::{glyph_color, text_paths, visit_pen_segments},
};
use resvg::tiny_skia::{BlendMode, Color, LineCap};
use std::{fmt::Write, sync::atomic::AtomicBool};

const MAX_GEOMETRY_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERASERS_PER_LAYER: usize = 12;

pub(super) fn page_svg(
    page: &Page,
    overview: RenderOptions,
    background: Option<BackgroundImage>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let size = RenderOptions::for_page(page);
    let dimensions = [size.width, size.height];
    let mut svg = vector::page_svg(size.width, size.height);
    let layers = || page.layers.iter().filter(|layer| layer.visible);
    if !layers().all(can_retain_layer) {
        let (png, _, _) = super::render_model(page, overview, background.as_ref(), cancel)?;
        vector::append_png(&mut svg, dimensions, &png);
    } else {
        if let Some(background) = background {
            let image =
                image::RgbaImage::from_raw(background.width, background.height, background.rgba)
                    .ok_or("Invalid reMarkable paper raster")?;
            vector::append_raster(&mut svg, dimensions, image)?;
        }
        let _ = write!(
            svg,
            "<g transform='translate({} 0)'>",
            f64::from(size.width) / 2.0
        );
        for (index, layer) in layers().enumerate() {
            write_layer(&mut svg, layer, dimensions, index, cancel)?;
        }
        svg.push_str("</g>");
        if let Some(text) = text_paths(page).map_err(|error| error.to_string())? {
            svg.push_str(&text);
        }
    }
    svg.push_str("</svg>");
    budget(&svg)?;
    Ok(svg)
}

fn can_retain_layer(layer: &Layer) -> bool {
    let gestures = layer
        .lines
        .iter()
        .filter(|line| is_eraser(line))
        .collect::<Vec<_>>();
    if gestures.is_empty() {
        return true;
    }
    // Legacy brush erasers provide their stroke width. Area-selection gestures,
    // newer unresolved operations and multiply/erase interactions lack a
    // qualified retained replay, so use the existing page preview.
    gestures.len() <= MAX_ERASERS_PER_LAYER
        && layer.glyphs.is_empty()
        && !layer.lines.iter().any(|line| {
            matches!(
                line.tool,
                Pen::HighlighterV1 | Pen::HighlighterV2 | Pen::Shader
            )
        })
        && gestures.iter().all(|line| {
            line.tool == Pen::Eraser
                && line.timestamp == CrdtId::SENTINEL
                && line.points.first().is_some_and(|first| {
                    first.width > 0 && line.points.iter().all(|point| point.width == first.width)
                })
        })
}

fn is_eraser(line: &Line) -> bool {
    matches!(line.tool, Pen::Eraser | Pen::EraserAreaSelect)
}

fn write_layer(
    svg: &mut String,
    layer: &Layer,
    dimensions: [u32; 2],
    index: usize,
    cancel: &AtomicBool,
) -> Result<(), String> {
    for glyph in &layer.glyphs {
        let (color, alpha) = color_attributes(glyph_color(glyph));
        for rect in &glyph.rectangles {
            let _ = write!(
                svg,
                "<rect x='{}' y='{}' width='{}' height='{}' fill='{color}' fill-opacity='{alpha}' style='mix-blend-mode:multiply'/>",
                rect.x, rect.y, rect.width, rect.height
            );
        }
        budget(svg)?;
    }
    let erasings = write_masks(svg, layer, dimensions, index)?;
    for (epoch, ink) in layer.lines.split(is_eraser).enumerate() {
        if epoch < erasings {
            let _ = write!(svg, "<g mask='url(#rm-cut-{index}-{epoch})'>");
        }
        for line in ink {
            check_cancel(cancel)?;
            write_stroke(svg, line)?;
        }
        if epoch < erasings {
            svg.push_str("</g>");
        }
    }
    Ok(())
}

fn write_masks(
    svg: &mut String,
    layer: &Layer,
    [width, height]: [u32; 2],
    index: usize,
) -> Result<usize, String> {
    let erasers: Vec<_> = layer.lines.iter().filter(|line| is_eraser(line)).collect();
    if erasers.is_empty() {
        return Ok(0);
    }
    let left = -f64::from(width) * 0.5;
    svg.push_str("<defs>");
    for (cut, line) in erasers.iter().enumerate() {
        let Some(first) = line.points.first() else {
            return Err("Empty reMarkable eraser".into());
        };
        let brush = f32::from(first.width) / 4.0;
        let _ = write!(
            svg,
            "<path id='rm-erase-{index}-{cut}' fill='none' stroke='black' stroke-width='{brush}' stroke-linecap='round' stroke-linejoin='round' d='"
        );
        write_points(svg, &line.points)?;
        svg.push_str("'/>");
    }
    for epoch in 0..erasers.len() {
        let _ = write!(
            svg,
            "<mask id='rm-cut-{index}-{epoch}' mask-type='luminance' maskUnits='userSpaceOnUse' x='{left}' y='0' width='{width}' height='{height}'>\
            <rect x='{left}' width='{width}' height='{height}' fill='white'/>"
        );
        // A brush erases earlier paint on its own layer only. Masks are
        // siblings so a busy history does not nest renderer surfaces.
        for cut in epoch..erasers.len() {
            let _ = write!(svg, "<use xlink:href='#rm-erase-{index}-{cut}'/>");
        }
        svg.push_str("</mask>");
    }
    svg.push_str("</defs>");
    budget(svg)?;
    Ok(erasers.len())
}

fn write_stroke(svg: &mut String, line: &Line) -> Result<(), String> {
    visit_pen_segments(line, |points, color, stroke, blend| {
        if svg.len() > MAX_GEOMETRY_BYTES {
            return;
        }
        let (color, alpha) = color_attributes(color);
        let cap = if stroke.line_cap == LineCap::Square {
            "square"
        } else {
            "round"
        };
        let style = if blend == BlendMode::Multiply {
            " style='mix-blend-mode:multiply'"
        } else {
            ""
        };
        let _ = write!(
            svg,
            "<path fill='none' stroke='{color}' stroke-opacity='{alpha}' stroke-width='{}' stroke-linecap='{cap}' stroke-linejoin='round'{style} d='",
            stroke.width
        );
        let _ = write_points(svg, points);
        svg.push_str("'/>");
    });
    budget(svg)
}

fn write_points(svg: &mut String, points: &[Point]) -> Result<(), String> {
    let [first, rest @ ..] = points else {
        return Ok(());
    };
    let _ = write!(svg, "M{} {}", first.x, first.y);
    let tail = if rest.is_empty() { points } else { rest };
    for point in tail {
        let _ = write!(svg, "L{} {}", point.x, point.y);
        budget(svg)?;
    }
    Ok(())
}

fn budget(svg: &str) -> Result<(), String> {
    if svg.len() > MAX_GEOMETRY_BYTES {
        return Err("reMarkable page detail exceeds the geometry budget".into());
    }
    Ok(())
}

fn color_attributes(color: Color) -> (String, f32) {
    let rgb = color.to_color_u8();
    (
        format!("#{:02x}{:02x}{:02x}", rgb.red(), rgb.green(), rgb.blue()),
        color.alpha(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{notebook::RetainedPage, page_region::PageRegion};
    use std::{fs::File, path::Path};

    #[test]
    fn retained_regions_follow_the_existing_pen_model_and_text_on_real_pages() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/notebooks/remarkable");
        let cancel = AtomicBool::new(false);
        for name in [
            "legacy-real/test_v5_advent_of_code.rm",
            "rmc/erasers.rm",
            "rmc/keyboard-checkboxes-and-bullets.rm",
            "rmc/writing_tools.rm",
            "rmscene/More_color_highlight_shader_v3.15.4.2.rm",
            "tablet-thumbnails/quicksheets-p2.rm",
        ] {
            let source = root.join(name);
            let (overview, width, height) =
                super::super::preview::render(&mut File::open(&source).unwrap(), 1, 512, &cancel)
                    .unwrap();
            let svg = super::super::preview::retain_page(
                &mut File::open(&source).unwrap(),
                1,
                512,
                &cancel,
            )
            .unwrap();
            assert!(svg.contains("<path"), "{name}");
            assert!(!svg.contains("<image") && !svg.contains("<text"), "{name}");
            let scene = RetainedPage::new(&svg, &cancel).unwrap();
            let expected = image::load_from_memory(&overview).unwrap().to_rgba8();
            let dimensions = [width, height];
            let actual = scene
                .render_region(
                    PageRegion {
                        page: dimensions,
                        size: dimensions,
                        origin: [0, 0],
                    },
                    &cancel,
                )
                .unwrap();
            let error = actual
                .data()
                .iter()
                .zip(expected.as_raw())
                .map(|(a, b)| f64::from(a.abs_diff(*b)))
                .sum::<f64>()
                / actual.data().len() as f64;
            assert!(error < 1.0, "{name}: mean channel error {error}");
        }
    }

    #[test]
    fn legacy_brush_eraser_cuts_earlier_ink_on_its_layer_only() {
        use remarkable_rm::PenColor;
        let trace = |tool, color_rgba, endpoints: [(f32, f32); 2]| Line {
            tool,
            color: PenColor::Black,
            color_rgba: Some(color_rgba),
            thickness_scale: 3.0,
            starting_length: 0.0,
            points: endpoints
                .into_iter()
                .map(|(x, y)| Point {
                    x,
                    y,
                    speed: 0,
                    width: 40,
                    direction: 0,
                    pressure: 255,
                })
                .collect(),
            timestamp: CrdtId::SENTINEL,
            move_id: None,
        };
        let layer = |lines| Layer {
            node_id: CrdtId::SENTINEL,
            name: String::new(),
            visible: true,
            lines,
            glyphs: Vec::new(),
        };
        let page = Page {
            layers: vec![
                layer(vec![trace(
                    Pen::FinelinerV1,
                    [0, 0, 255, 255],
                    [(-40.0, 50.0), (40.0, 50.0)],
                )]),
                layer(vec![
                    trace(
                        Pen::FinelinerV1,
                        [0, 0, 0, 255],
                        [(-40.0, 50.0), (40.0, 50.0)],
                    ),
                    trace(Pen::Eraser, [255; 4], [(0.0, 30.0), (0.0, 70.0)]),
                    trace(
                        Pen::FinelinerV1,
                        [255, 0, 0, 255],
                        [(-15.0, 70.0), (15.0, 70.0)],
                    ),
                ]),
            ],
            paper_size: Some((100, 100)),
            text: None,
        };
        let cancel = AtomicBool::new(false);
        let svg = page_svg(&page, RenderOptions::for_page(&page), None, &cancel).unwrap();
        let scene = RetainedPage::new(&svg, &cancel).unwrap();
        let window = scene
            .render_region(
                PageRegion {
                    page: [1000, 1000],
                    origin: [320, 400],
                    size: [300, 380],
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(window.pixel(30, 100).unwrap().red(), 0);
        assert_eq!(window.pixel(30, 100).unwrap().blue(), 0);
        assert_eq!(window.pixel(180, 100).unwrap().blue(), 255);
        assert_eq!(window.pixel(180, 300).unwrap().red(), 255);
        assert_eq!(window.pixel(180, 300).unwrap().blue(), 0);
    }
}
