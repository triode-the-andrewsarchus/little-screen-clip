//! Convert the dedicated legacy Rust parser's page model to the native renderer.
//! No parser implementation is copied or compiled from another crate's sources.
use remarkable_lines::{
    RemarkableFile,
    other::{line::Line as LegacyLine, point::Point as LegacyPoint},
};
use remarkable_rm::{CrdtId, Layer, Line, Page, Pen, PenColor, Point};

pub(super) fn parse(bytes: &[u8]) -> Result<Page, String> {
    let RemarkableFile::Other { pages, .. } =
        RemarkableFile::read(bytes).map_err(|error| error.to_string())?
    else {
        return Err("Expected a legacy reMarkable raw page".into());
    };
    let [source] =
        <[_; 1]>::try_from(pages).map_err(|_| "Expected one legacy reMarkable raw page")?;
    if source.layers.len() > 4096 {
        return Err("reMarkable layers exceed the preview limit".into());
    }
    let mut total = 0usize;
    let mut strokes = 0usize;
    let mut layers = Vec::new();
    for (index, layer) in source.layers.into_iter().enumerate() {
        strokes += layer.lines.len();
        if strokes > super::MAX_BLOCKS {
            return Err("reMarkable strokes exceed the preview limit".into());
        }
        let mut lines = Vec::new();
        for line in layer.lines {
            total += line.points.len();
            if total > super::MAX_POINTS {
                return Err("reMarkable page exceeds the point preview limit".into());
            }
            lines.push(convert_line(line)?);
        }
        layers.push(Layer {
            node_id: CrdtId {
                author: 0,
                seq: index as u64 + 2,
            },
            name: String::new(),
            visible: true,
            lines,
            glyphs: Vec::new(),
        });
    }
    Ok(Page {
        layers,
        text: None,
        paper_size: None,
    })
}

fn convert_line(line: LegacyLine) -> Result<Line, String> {
    Ok(Line {
        tool: pen(line.tool)?,
        color: color(line.color),
        color_rgba: None,
        thickness_scale: f64::from(line.brush_size),
        starting_length: 0.0,
        points: line
            .points
            .into_iter()
            .map(point)
            .collect::<Result<_, _>>()?,
        timestamp: CrdtId::SENTINEL,
        move_id: None,
    })
}

fn pen(tool: remarkable_lines::shared::tool::Tool) -> Result<Pen, String> {
    use remarkable_lines::shared::tool::Tool;
    Ok(match tool {
        Tool::Brush => Pen::PaintbrushV1,
        Tool::Pencil => Pen::PencilV1,
        Tool::BallPoint => Pen::BallpointV1,
        Tool::Marker => Pen::MarkerV1,
        Tool::FineLiner => Pen::FinelinerV1,
        Tool::Highlighter => Pen::HighlighterV1,
        Tool::Eraser => Pen::Eraser,
        Tool::MechanicalPencil => Pen::MechanicalPencilV1,
        Tool::EraseArea => Pen::EraserAreaSelect,
        Tool::Calligraphy => Pen::Calligraphy,
        Tool::EraseAll | Tool::SelectionBrush => {
            return Err("Unsupported legacy reMarkable selection/erase-all stroke".into());
        }
    })
}

fn color(value: remarkable_lines::shared::pen_color::PenColor) -> PenColor {
    use remarkable_lines::shared::pen_color::PenColor as Legacy;
    match value {
        Legacy::Black => PenColor::Black,
        Legacy::Grey => PenColor::Gray,
        Legacy::White => PenColor::White,
        Legacy::Yellow => PenColor::Yellow,
        Legacy::Green => PenColor::Green,
        Legacy::Pink => PenColor::Pink,
        Legacy::Blue => PenColor::Blue,
        Legacy::Red => PenColor::Red,
        Legacy::GreyOverlap => PenColor::GrayOverlap,
    }
}

fn point(value: LegacyPoint) -> Result<Point, String> {
    if ![
        value.x,
        value.y,
        value.speed,
        value.direction,
        value.width,
        value.pressure,
    ]
    .into_iter()
    .all(|value| super::coordinate(f64::from(value)))
        || value.speed < 0.0
        || value.width < 0.0
        || !(0.0..=1.0).contains(&value.pressure)
    {
        return Err("Invalid legacy reMarkable point geometry".into());
    }
    // Legacy coordinates have a top-left origin; the renderer's v6 model uses
    // a top-center origin. Its packed point units are quarter-pixels for width/
    // speed, and 0..255 for direction and pressure. Round once when normalizing.
    Ok(Point {
        x: value.x - remarkable_rm::DEFAULT_WIDTH as f32 / 2.0,
        y: value.y,
        speed: (value.speed * 4.0).round().clamp(0.0, f32::from(u16::MAX)) as u16,
        width: (value.width * 4.0).round().clamp(0.0, f32::from(u16::MAX)) as u16,
        direction: (value.direction * 255.0 / std::f32::consts::TAU)
            .round()
            .clamp(0.0, 255.0) as u8,
        pressure: (value.pressure * 255.0).round() as u8,
    })
}
