//! Gerber RS-274X/X2 and Excellon drill parsing, attribute inspection, and bounded rendering.

use super::{
    allocate_canvas, bounded_source_text, check_cancelled, extend_arc_bounds, pixmap_to_thumbnail,
    update_bounds,
};
use crate::ImageThumbnail;
use lib_gerber_edit::LayerCorners;
use lib_gerber_edit::excellon_format::{
    Command as ExcellonCommand, ExcellonLayerData, GeometricCode, Mode, parse_excellon,
};
use lib_gerber_edit::flatten::{
    Affine2, ExpandedObject, FlatLayer, FlattenOptions, GraphicsObject, Point, Polarity,
    ResourceLimits, Segment, Shape,
};
use lib_gerber_edit::gerber::GerberLayerData;
use lib_gerber_edit::gerber_parser;
use lib_gerber_edit::gerber_types::{
    Aperture, Command, CommentContent, CoordinateFormat, CoordinateMode, CopperType, ExtendedCode,
    ExtendedPosition, FileAttribute, FileFunction, FilePolarity, FunctionCode, GCode, Part,
    Profile, StandardComment, Unit, ZeroOmission,
};
use lib_gerber_edit::layer::LayerType;
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::TAU;
use std::io::{BufReader, Cursor};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

const MAX_EXPANDED_OPS: usize = 400_000;
const MAX_EXCELLON_REPEATS: u32 = 250_000;
const MAX_OVERLAY_POLYLINES: usize = 24_000;
const ARC_TOLERANCE_MM: f64 = 0.01;
const BG_COLOR: [u8; 4] = [18, 22, 26, 255];

const DRILL_PALETTE: [[u8; 3]; 8] = [
    [94, 212, 224],
    [244, 178, 72],
    [138, 224, 118],
    [224, 118, 176],
    [146, 176, 246],
    [240, 126, 98],
    [198, 152, 240],
    [232, 222, 126],
];

/// Aperture metadata captured for Gerber inspection.
#[derive(Debug, Clone)]
pub struct GerberApertureInfo {
    pub d_code: i32,
    pub kind: String,
    pub dimensions: String,
    pub flash_count: usize,
    pub stroke_count: usize,
}

/// Drill tool metadata captured for Excellon inspection.
#[derive(Debug, Clone)]
pub struct GerberToolInfo {
    pub number: u32,
    pub diameter_mm: f64,
    pub diameter_native: f64,
    pub hit_count: usize,
    pub slot_count: usize,
}

/// Flattened vector polyline or closed contour stored for interactive canvas overlays.
#[derive(Debug, Clone)]
pub struct VectorPolyline {
    pub points: Vec<[f64; 2]>,
    pub stroke_width_mm: f64,
    pub clear: bool,
    pub closed: bool,
    pub aperture: Option<i32>,
}

/// Decoded Gerber RS-274X/X2 or Excellon drill layer.
#[derive(Debug, Clone)]
pub struct GerberDocument {
    pub is_excellon: bool,
    pub layer_type: String,
    pub layer_color: [u8; 3],
    pub units: String,
    pub coordinate_format: String,
    pub polarity: String,
    pub copper_thickness: Option<String>,
    pub attributes: Vec<(String, String)>,
    pub apertures: Vec<GerberApertureInfo>,
    pub tools: Vec<GerberToolInfo>,
    pub aperture_count: usize,
    pub macro_count: usize,
    pub flash_count: usize,
    pub stroke_count: usize,
    pub region_count: usize,
    pub clear_count: usize,
    pub hole_count: usize,
    pub slot_count: usize,
    pub arc_count: usize,
    pub bounds_mm: [f64; 4],
    pub polylines: Vec<VectorPolyline>,
    pub flat: FlatLayer,
    pub issues: Vec<String>,
    pub source_excerpt: String,
}

impl GerberDocument {
    /// Parses an RS-274X / X2 Gerber byte buffer.
    pub fn parse_gerber(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "Gerber parse cancelled")?;
        let text_lossy = bounded_source_text(bytes);
        let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            gerber_parser::parse(BufReader::new(Cursor::new(bytes)))
                .map_err(|(_, error)| format!("Gerber parse error: {error}"))
        }))
        .map_err(|_| "Gerber parser failed on malformed input".to_string())??;
        check_cancelled(cancelled, "Gerber parse cancelled")?;

        let meta = inspect_gerber_header(path, &parsed);
        let gerber = GerberLayerData::new(meta.name_layer_type, parsed)
            .map_err(|error| format!("Invalid Gerber data: {error}"))?;
        check_cancelled(cancelled, "Gerber parse cancelled")?;

        let mut issues = gerber.parse_errors.clone();
        let flat = gerber
            .flatten_with(&bounded_flatten_options())
            .map_err(|error| format!("Gerber geometry limit exceeded: {error}"))?;
        check_cancelled(cancelled, "Gerber parse cancelled")?;

        let macro_count = gerber.macros.len();
        let aperture_defs = &gerber.apertures;
        let (corners_lo, corners_hi) = gerber.get_corners();
        let stats = collect_flat_stats(
            &flat,
            Some(aperture_defs),
            corners_lo,
            corners_hi,
            cancelled,
        )?;

        for issue in flat.issues.iter().take(32) {
            issues.push(issue.to_string());
        }
        let layer_type = resolve_layer_type_label(gerber.layer_type, meta.x2_function.as_ref());
        let polarity = format_polarity_label(
            gerber.is_negative(),
            meta.x2_polarity,
            stats.clear_count > 0,
        );

        Ok(Self {
            is_excellon: false,
            layer_type,
            layer_color: layer_color_for_type(&gerber.layer_type),
            units: meta.units,
            coordinate_format: meta.coordinate_format,
            polarity,
            copper_thickness: meta.copper_thickness,
            attributes: meta.attributes,
            apertures: stats.apertures,
            tools: Vec::new(),
            aperture_count: gerber.apertures.len(),
            macro_count,
            flash_count: stats.flash_count,
            stroke_count: stats.stroke_count,
            region_count: stats.region_count,
            clear_count: stats.clear_count,
            hole_count: 0,
            slot_count: 0,
            arc_count: stats.arc_count,
            bounds_mm: stats.bounds_mm,
            polylines: stats.polylines,
            flat,
            issues,
            source_excerpt: text_lossy,
        })
    }

    /// Parses an Excellon drill/route byte buffer (falling back to Gerber drill maps if valid Gerber).
    pub fn parse_excellon(
        path: &Path,
        bytes: &[u8],
        cancelled: &AtomicBool,
    ) -> Result<Self, String> {
        check_cancelled(cancelled, "Excellon parse cancelled")?;
        check_excellon_repeat_budget(bytes, MAX_EXCELLON_REPEATS, cancelled)?;
        let cleaned = strip_excellon_park_tool(bytes);
        match parse_excellon(BufReader::new(Cursor::new(cleaned.as_ref()))) {
            Ok(excellon) if has_excellon_content(&excellon) || !looks_like_gerber(bytes) => {
                build_excellon_document(&excellon, bytes, cancelled)
            }
            _ if looks_like_gerber(bytes) => Self::parse_gerber(path, bytes, cancelled),
            Ok(excellon) => build_excellon_document(&excellon, bytes, cancelled),
            Err(error) => Err(format!("Excellon parse error: {error}")),
        }
    }

    pub fn summary_line(&self) -> String {
        let w = (self.bounds_mm[2] - self.bounds_mm[0]).max(0.0);
        let h = (self.bounds_mm[3] - self.bounds_mm[1]).max(0.0);
        if self.is_excellon {
            format!(
                "Excellon · {} tools · {} holes · {:.1}×{:.1} mm",
                self.tools.len(),
                self.hole_count + self.slot_count,
                w,
                h
            )
        } else {
            format!(
                "{} · {} apertures · {} obj · {:.1}×{:.1} mm",
                self.layer_type,
                self.aperture_count,
                self.flash_count + self.stroke_count + self.region_count,
                w,
                h
            )
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "Gerber render cancelled")?;
        let bg = Color::from_rgba8(BG_COLOR[0], BG_COLOR[1], BG_COLOR[2], BG_COLOR[3]);
        let (mut image, transform, scale) = allocate_canvas(self.bounds_mm, max_size, bg)
            .ok_or_else(|| "Could not allocate Gerber preview".to_string())?;
        let mut layer_canvas = Pixmap::new(image.width(), image.height())
            .ok_or_else(|| "Could not allocate Gerber layer buffer".to_string())?;

        render_flat_objects(
            &mut layer_canvas,
            &self.flat,
            self.layer_color,
            self.is_excellon,
            transform,
            scale,
            cancelled,
        )?;

        // X2 FilePolarity describes the layer's material. Only legacy IPNEG
        // complements the image, after dark and clear objects are composited.
        if self.flat.negative {
            let [r, g, b] = self.layer_color;
            let mut negative = Pixmap::new(image.width(), image.height())
                .ok_or_else(|| "Could not allocate negative Gerber layer buffer".to_string())?;
            negative.fill(Color::from_rgba8(r, g, b, 255));
            negative.draw_pixmap(
                0,
                0,
                layer_canvas.as_ref(),
                &PixmapPaint {
                    blend_mode: BlendMode::DestinationOut,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
            layer_canvas = negative;
        }

        image.draw_pixmap(
            0,
            0,
            layer_canvas.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        Ok(pixmap_to_thumbnail(image))
    }
}

/// Fast content check used to separate RS-274X Gerber `.gbr` files from binary/random `.gbr`.
pub fn looks_like_gerber(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.contains(&0) {
        return false;
    }
    let prefix = &bytes[..bytes.len().min(8192)];
    let text = String::from_utf8_lossy(prefix);
    let mut hits = 0_u8;
    for marker in [
        "%FS", "%MO", "%AD", "%AM", "%TF", "%TA", "%LP", "G04", "G36*", "G75*", "M02*", "D10*",
    ] {
        if text.contains(marker) {
            hits += 1;
        }
    }
    hits >= 2 || (hits >= 1 && text.contains("D0") && text.contains('*'))
}

struct GerberHeaderMeta {
    name_layer_type: LayerType,
    x2_function: Option<FileFunction>,
    x2_polarity: Option<FilePolarity>,
    units: String,
    coordinate_format: String,
    copper_thickness: Option<String>,
    attributes: Vec<(String, String)>,
}

fn inspect_gerber_header(path: &Path, doc: &gerber_parser::GerberDoc) -> GerberHeaderMeta {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let name_layer_type =
        LayerType::from_file_name(file_name).unwrap_or(LayerType::UndefinedGerber);

    let units = match doc.units {
        Some(Unit::Millimeters) => "Millimeters (mm)".to_string(),
        Some(Unit::Inches) => "Inches (in)".to_string(),
        None => "Unspecified (mm assumed)".to_string(),
    };
    let coordinate_format = doc
        .format_specification
        .map(format_gerber_coord_spec)
        .unwrap_or_else(|| "Unspecified".to_string());

    let mut x2_function = None;
    let mut x2_polarity = None;
    let mut copper_thickness = None;
    let mut attributes = Vec::new();

    for cmd in doc.commands.iter().flatten() {
        match cmd {
            Command::ExtendedCode(ExtendedCode::FileAttribute(attr))
            | Command::FunctionCode(FunctionCode::GCode(GCode::Comment(
                CommentContent::Standard(StandardComment::FileAttribute(attr)),
            ))) => {
                collect_file_attr(
                    attr,
                    &mut x2_function,
                    &mut x2_polarity,
                    &mut copper_thickness,
                    &mut attributes,
                );
            }
            Command::FunctionCode(FunctionCode::GCode(GCode::Comment(CommentContent::String(
                text,
            )))) if copper_thickness.is_none() => {
                copper_thickness = extract_copper_thickness_hint(text);
            }
            _ => {}
        }
    }

    GerberHeaderMeta {
        name_layer_type,
        x2_function,
        x2_polarity,
        units,
        coordinate_format,
        copper_thickness,
        attributes,
    }
}

fn collect_user_defined_attr(
    name: &str,
    values: &[String],
    copper_thickness: &mut Option<String>,
) -> (String, String) {
    let joined = values.join(", ");
    let lower = name.to_ascii_lowercase();
    if copper_thickness.is_none()
        && !joined.is_empty()
        && (lower.contains("thickness") || lower.contains("copper") || lower.contains("weight"))
    {
        *copper_thickness = Some(joined.clone());
    }
    (format!("TF.{name}"), joined)
}

fn collect_file_attr(
    attr: &FileAttribute,
    x2_function: &mut Option<FileFunction>,
    x2_polarity: &mut Option<FilePolarity>,
    copper_thickness: &mut Option<String>,
    attributes: &mut Vec<(String, String)>,
) {
    let (key, val) = match attr {
        FileAttribute::FileFunction(ff) => {
            *x2_function = Some(ff.clone());
            ("TF.FileFunction".to_string(), format_file_function(ff))
        }
        FileAttribute::FilePolarity(pol) => {
            *x2_polarity = Some(*pol);
            let label = match pol {
                FilePolarity::Positive => "Positive",
                FilePolarity::Negative => "Negative",
            };
            ("TF.FilePolarity".to_string(), label.to_string())
        }
        FileAttribute::Part(part) => ("TF.Part".to_string(), format_part(part)),
        FileAttribute::GenerationSoftware(gs) => (
            "TF.GenerationSoftware".to_string(),
            gs.version.as_deref().map_or_else(
                || format!("{}, {}", gs.vendor, gs.application),
                |ver| format!("{}, {}, {}", gs.vendor, gs.application, ver),
            ),
        ),
        FileAttribute::CreationDate(dt) => ("TF.CreationDate".to_string(), dt.to_rfc3339()),
        FileAttribute::ProjectId { id, uuid, revision } => (
            "TF.ProjectId".to_string(),
            format!("{id} (rev {revision}, {uuid})"),
        ),
        FileAttribute::SameCoordinates(Some(ident)) => {
            ("TF.SameCoordinates".to_string(), format!("{ident:?}"))
        }
        FileAttribute::SameCoordinates(None) => ("TF.SameCoordinates".to_string(), "Yes".into()),
        FileAttribute::Md5(hash) => ("TF.MD5".to_string(), hash.clone()),
        FileAttribute::UserDefined { name, values } => {
            collect_user_defined_attr(name, values, copper_thickness)
        }
    };
    if attributes.len() < 64 {
        attributes.push((key, val));
    }
}

fn format_part(part: &Part) -> String {
    match part {
        Part::Single => "Single PCB".to_string(),
        Part::Array => "Array / Panel".to_string(),
        Part::FabricationPanel => "Fabrication Panel".to_string(),
        Part::Coupon => "Test Coupon".to_string(),
        Part::Other(desc) => format!("Other ({desc})"),
    }
}

fn extended_pos_label(pos: ExtendedPosition) -> &'static str {
    match pos {
        ExtendedPosition::Top => "Top",
        ExtendedPosition::Bottom => "Bottom",
        ExtendedPosition::Inner => "Inner",
    }
}

fn copper_type_label(ct: CopperType) -> &'static str {
    match ct {
        CopperType::Plane => "Plane",
        CopperType::Signal => "Signal",
        CopperType::Mixed => "Mixed",
        CopperType::Hatched => "Hatched",
    }
}

fn format_copper_function(
    layer: i32,
    pos: ExtendedPosition,
    copper_type: Option<CopperType>,
    for_layer_label: bool,
) -> String {
    let side = extended_pos_label(pos);
    match (for_layer_label, copper_type) {
        (true, Some(ct)) => format!("Copper {side} (L{layer} · {})", copper_type_label(ct)),
        (true, None) => format!("Copper {side} (L{layer})"),
        (false, Some(ct)) => format!("Copper, L{layer}, {side}, {ct:?}"),
        (false, None) => format!("Copper, L{layer}, {side}"),
    }
}

fn format_file_function(ff: &FileFunction) -> String {
    match ff {
        FileFunction::Copper {
            layer,
            pos,
            copper_type,
        } => format_copper_function(*layer, *pos, *copper_type, false),
        FileFunction::SolderMask { pos, .. } => format!("Solder Mask ({pos:?})"),
        FileFunction::Legend { pos, .. } => format!("Silkscreen / Legend ({pos:?})"),
        FileFunction::Paste(pos) => format!("Solder Paste ({pos:?})"),
        FileFunction::Profile(Some(Profile::Plated)) => "Profile (Plated)".into(),
        FileFunction::Profile(Some(Profile::NonPlated)) => "Profile (Non-Plated)".into(),
        FileFunction::Profile(None) => "Profile / Outline".into(),
        FileFunction::Plated {
            from_layer,
            to_layer,
            drill,
            ..
        } => format!("Plated Drill L{from_layer}-L{to_layer} ({drill:?})"),
        FileFunction::NonPlated {
            from_layer,
            to_layer,
            drill,
            ..
        } => format!("Non-Plated Drill L{from_layer}-L{to_layer} ({drill:?})"),
        FileFunction::DrillMap => "Drill Map".into(),
        FileFunction::KeepOut(pos) => format!("Keep-Out ({pos:?})"),
        FileFunction::VCut(_) => "V-Cut".into(),
        FileFunction::Other(s) => format!("Other ({s})"),
        other => format!("{other:?}"),
    }
}

fn format_gerber_coord_spec(spec: CoordinateFormat) -> String {
    let zo_code = match spec.zero_omission {
        ZeroOmission::Leading => "L",
        ZeroOmission::Trailing => "T",
    };
    let cm_code = match spec.coordinate_mode {
        CoordinateMode::Absolute => "A",
        CoordinateMode::Incremental => "I",
    };
    let zo_word = match spec.zero_omission {
        ZeroOmission::Leading => "Leading",
        ZeroOmission::Trailing => "Trailing",
    };
    let cm_word = match spec.coordinate_mode {
        CoordinateMode::Absolute => "Absolute",
        CoordinateMode::Incremental => "Incremental",
    };
    format!(
        "FS{zo_code}{cm_code}X{}{}Y{}{} ({}.{} {zo_word} {cm_word})",
        spec.integer, spec.decimal, spec.integer, spec.decimal, spec.integer, spec.decimal
    )
}

fn extract_copper_thickness_hint(comment: &str) -> Option<String> {
    let lower = comment.to_ascii_lowercase();
    if !(lower.contains("thickness")
        || lower.contains("copper weight")
        || lower.contains("cu weight")
        || lower.contains("cu_thickness"))
    {
        return None;
    }
    let after = comment
        .split_once([':', '='])
        .map(|(_, rhs)| rhs.trim())
        .unwrap_or_else(|| comment.trim());
    if after.is_empty() || after.len() > 64 {
        return None;
    }
    Some(after.to_string())
}

fn copper_or_surface_label(ty: LayerType) -> Option<&'static str> {
    Some(match ty {
        LayerType::Top => "Copper Top",
        LayerType::Bottom => "Copper Bottom",
        LayerType::MaskTop => "Solder Mask Top",
        LayerType::MaskBottom => "Solder Mask Bottom",
        LayerType::SilkScreenTop => "Silkscreen Top",
        LayerType::SilkScreenBottom => "Silkscreen Bottom",
        LayerType::PasteTop => "Solder Paste Top",
        LayerType::PasteBottom => "Solder Paste Bottom",
        _ => return None,
    })
}

fn mechanical_or_aux_label(ty: LayerType) -> Option<&'static str> {
    Some(match ty {
        LayerType::Dimensions | LayerType::Milling => "Board Outline / Profile",
        LayerType::VCut => "V-Cut",
        LayerType::KeepOut => "Keep-Out",
        LayerType::CourtyardTop => "Courtyard Top",
        LayerType::CourtyardBottom => "Courtyard Bottom",
        LayerType::SidePlating => "Side Plating",
        LayerType::Drill => "Drill Map",
        LayerType::Info => "Info",
        _ => return None,
    })
}

fn resolve_layer_type_label(ty: LayerType, x2: Option<&FileFunction>) -> String {
    if let Some(FileFunction::Copper {
        layer,
        pos,
        copper_type,
    }) = x2
    {
        return format_copper_function(*layer, *pos, *copper_type, true);
    }
    if let Some(label) = copper_or_surface_label(ty).or_else(|| mechanical_or_aux_label(ty)) {
        return label.into();
    }
    match ty {
        LayerType::Inner(n) => format!("Copper Inner L{n}"),
        LayerType::UndefinedGerber => {
            x2.map_or_else(|| "Gerber Layer".into(), format_file_function)
        }
        _ => "Gerber Layer".into(),
    }
}

fn format_polarity_label(
    is_neg_ip: bool,
    x2_pol: Option<FilePolarity>,
    has_clear_regions: bool,
) -> String {
    let base = match (is_neg_ip, x2_pol) {
        (true, _) | (_, Some(FilePolarity::Negative)) => "Negative",
        (false, Some(FilePolarity::Positive)) => "Positive",
        (false, None) => "Positive",
    };
    if has_clear_regions {
        format!("{base} (with Clear polarity cutouts)")
    } else {
        base.to_string()
    }
}

// The dependency expands repeat hits during parsing, before flatten limits apply.
fn check_excellon_repeat_budget(
    bytes: &[u8],
    mut remaining: u32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Excellon source is not valid UTF-8")?;
    for (index, line) in text.lines().enumerate() {
        if index.is_multiple_of(1024) {
            check_cancelled(cancelled, "Excellon parse cancelled")?;
        }
        let Some(tail) = line.trim().strip_prefix('R') else {
            continue;
        };
        let end = tail
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(tail.len());
        if let Ok(count) = tail[..end].parse::<u32>() {
            remaining = remaining
                .checked_sub(count)
                .ok_or("Excellon repeat hits exceed the preview limit")?;
        }
    }
    Ok(())
}

/// Strips KiCad/CAM `T0` / `T00` park-tool lines that `lib_gerber_edit` rejects as tool 0.
fn strip_excellon_park_tool(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return std::borrow::Cow::Borrowed(bytes);
    };
    let mut has_park_tool = false;
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("T0") || trimmed.eq_ignore_ascii_case("T00") {
            if !has_park_tool {
                out.reserve(text.len());
                // Lazily backfill everything processed before the first park-tool line.
                let prefix_len = line.as_ptr() as usize - text.as_ptr() as usize;
                out.push_str(&text[..prefix_len]);
                has_park_tool = true;
            }
            if line.ends_with('\n') {
                out.push('\n');
            }
        } else if has_park_tool {
            out.push_str(line);
        }
    }
    if has_park_tool {
        std::borrow::Cow::Owned(out.into_bytes())
    } else {
        std::borrow::Cow::Borrowed(bytes)
    }
}

fn has_excellon_content(data: &ExcellonLayerData) -> bool {
    !data.tools.is_empty() || !data.is_empty()
}

struct ExcellonCommandCounts {
    hit_by_tool: BTreeMap<u32, usize>,
    slot_by_tool: BTreeMap<u32, usize>,
    raw_arc_count: usize,
}

fn count_excellon_commands(excellon: &ExcellonLayerData) -> ExcellonCommandCounts {
    let mut counts = ExcellonCommandCounts {
        hit_by_tool: BTreeMap::new(),
        slot_by_tool: BTreeMap::new(),
        raw_arc_count: 0,
    };
    let mut current_tool: Option<u32> = None;
    let mut route_mode = Mode::DrillMode;

    for cmd in excellon.commands.iter().flatten() {
        match cmd {
            ExcellonCommand::Tool(id) => current_tool = Some(*id),
            ExcellonCommand::Geometric(GeometricCode::Mode(mode)) => route_mode = *mode,
            ExcellonCommand::Coordinate(..)
                if matches!(route_mode, Mode::DrillMode | Mode::CannedDrill) =>
            {
                if let Some(t) = current_tool {
                    *counts.hit_by_tool.entry(t).or_default() += 1;
                }
            }
            ExcellonCommand::Slot { .. } => {
                if let Some(t) = current_tool {
                    *counts.slot_by_tool.entry(t).or_default() += 1;
                }
            }
            ExcellonCommand::Arc { .. } => {
                counts.raw_arc_count += 1;
                if let Some(t) = current_tool {
                    *counts.slot_by_tool.entry(t).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    counts
}

fn build_excellon_document(
    excellon: &ExcellonLayerData,
    bytes: &[u8],
    cancelled: &AtomicBool,
) -> Result<GerberDocument, String> {
    let flat = excellon
        .flatten_with(&bounded_flatten_options())
        .map_err(|error| format!("Excellon geometry limit exceeded: {error}"))?;
    check_cancelled(cancelled, "Excellon parse cancelled")?;

    let (corners_lo, corners_hi) = excellon.get_corners();
    let stats = collect_flat_stats(&flat, None, corners_lo, corners_hi, cancelled)?;
    let counts = count_excellon_commands(excellon);

    let mut tools: Vec<GerberToolInfo> = excellon
        .tools
        .iter()
        .map(|(&number, &diameter_native)| GerberToolInfo {
            number,
            diameter_mm: match excellon.unit.unit {
                Unit::Millimeters => diameter_native,
                Unit::Inches => diameter_native * 25.4,
            },
            diameter_native,
            hit_count: counts.hit_by_tool.get(&number).copied().unwrap_or(0),
            slot_count: counts.slot_by_tool.get(&number).copied().unwrap_or(0),
        })
        .collect();
    tools.sort_by_key(|t| t.number);

    let mut issues: Vec<String> = excellon
        .commands
        .iter()
        .filter_map(|res| res.as_ref().err().map(|e| e.to_string()))
        .take(32)
        .collect();
    for issue in flat.issues.iter().take(16) {
        issues.push(issue.to_string());
    }

    let units = match excellon.unit.unit {
        Unit::Millimeters => "Millimeters (mm)".to_string(),
        Unit::Inches => "Inches (in)".to_string(),
    };

    Ok(GerberDocument {
        is_excellon: true,
        layer_type: "Excellon NC Drill".into(),
        layer_color: DRILL_PALETTE[0],
        units,
        coordinate_format: excellon.unit.to_string(),
        polarity: "Drill / Route".into(),
        copper_thickness: None,
        attributes: Vec::new(),
        apertures: Vec::new(),
        tools,
        aperture_count: excellon.tools.len(),
        macro_count: 0,
        flash_count: stats.flash_count,
        stroke_count: stats.stroke_count,
        region_count: stats.region_count,
        clear_count: stats.clear_count,
        hole_count: stats.flash_count,
        slot_count: stats.stroke_count,
        arc_count: counts.raw_arc_count.max(stats.arc_count),
        bounds_mm: stats.bounds_mm,
        polylines: stats.polylines,
        flat,
        issues,
        source_excerpt: bounded_source_text(bytes),
    })
}

fn bounded_flatten_options() -> FlattenOptions {
    FlattenOptions::default()
        .limits(
            ResourceLimits::default()
                .max_instances(250_000)
                .max_block_depth(32)
                .max_aperture_points(1_000_000),
        )
        .tolerance_mm(ARC_TOLERANCE_MM)
}

struct FlatStats {
    apertures: Vec<GerberApertureInfo>,
    flash_count: usize,
    stroke_count: usize,
    region_count: usize,
    clear_count: usize,
    arc_count: usize,
    bounds_mm: [f64; 4],
    polylines: Vec<VectorPolyline>,
}

struct FlatAccum {
    flash_count: usize,
    stroke_count: usize,
    region_count: usize,
    clear_count: usize,
    arc_count: usize,
    flash_by_code: BTreeMap<i32, usize>,
    stroke_by_code: BTreeMap<i32, usize>,
    polylines: Vec<VectorPolyline>,
}

fn transform_points(points: Vec<Point>, xf: &Affine2) -> Vec<[f64; 2]> {
    points.into_iter().map(|p| xf.apply(p)).collect()
}

fn path_bounds(path: &lib_gerber_edit::flatten::Path, bounds: &mut Option<[f64; 4]>) {
    let mut from = path.start;
    update_bounds(bounds, from[0], from[1]);
    for segment in &path.segments {
        let to = segment.end();
        update_bounds(bounds, to[0], to[1]);
        if let Segment::Arc { center, ccw, .. } = *segment {
            let start = (from[1] - center[1]).atan2(from[0] - center[0]);
            let end = (to[1] - center[1]).atan2(to[0] - center[0]);
            let direction = if ccw { 1.0 } else { -1.0 };
            let sweep = ((end - start) * direction).rem_euclid(TAU);
            let sweep = if sweep == 0.0 { TAU } else { sweep };
            let radius = (from[0] - center[0]).hypot(from[1] - center[1]);
            extend_arc_bounds(bounds, center, radius, start, start + direction * sweep);
        }
        from = to;
    }
}

fn local_object_bounds(
    object: &GraphicsObject,
    apertures: &BTreeMap<i32, [f64; 4]>,
) -> Option<[f64; 4]> {
    let aperture = object
        .aperture
        .and_then(|code| apertures.get(&code))
        .copied();
    let mut bounds = None;
    match &object.shape {
        Shape::Flash => aperture,
        Shape::Path(path) => {
            path_bounds(path, &mut bounds);
            bounds.map(|[x0, y0, x1, y1]| {
                let [left, bottom, right, top] = aperture.unwrap_or_default();
                [x0 + left, y0 + bottom, x1 + right, y1 + top]
            })
        }
        Shape::Region { contours } => {
            for path in contours {
                path_bounds(path, &mut bounds);
            }
            bounds
        }
        _ => None,
    }
}

fn record_flat_shape(
    acc: &mut FlatAccum,
    flat: &FlatLayer,
    expanded: &lib_gerber_edit::flatten::ExpandedObject<'_>,
) {
    let clear = expanded.polarity == Polarity::Clear;
    if clear {
        acc.clear_count += 1;
    }
    let aperture = expanded.object.aperture;
    let aperture_img = aperture.and_then(|code| flat.apertures.get(&code));

    match &expanded.object.shape {
        Shape::Flash => {
            acc.flash_count += 1;
            if let Some(code) = aperture {
                *acc.flash_by_code.entry(code).or_default() += 1;
            }
            if acc.polylines.len() < MAX_OVERLAY_POLYLINES
                && let Some(img) = aperture_img
            {
                push_flash_polylines(
                    &mut acc.polylines,
                    &img.shapes,
                    &expanded.transform,
                    clear,
                    aperture,
                );
            }
        }
        Shape::Path(path) => {
            acc.stroke_count += 1;
            acc.arc_count += path
                .segments
                .iter()
                .filter(|seg| matches!(seg, lib_gerber_edit::flatten::Segment::Arc { .. }))
                .count();
            if let Some(code) = aperture {
                *acc.stroke_by_code.entry(code).or_default() += 1;
            }
            if acc.polylines.len() < MAX_OVERLAY_POLYLINES {
                let width = aperture_img
                    .map(|img| aperture_stroke_width(&img.aperture))
                    .unwrap_or(0.05);
                let pts = transform_points(path.to_polyline(ARC_TOLERANCE_MM), &expanded.transform);
                if pts.len() >= 2 {
                    acc.polylines.push(VectorPolyline {
                        points: pts,
                        stroke_width_mm: width,
                        clear,
                        closed: false,
                        aperture,
                    });
                }
            }
        }
        Shape::Region { contours } => {
            acc.region_count += 1;
            for contour in contours {
                if acc.polylines.len() >= MAX_OVERLAY_POLYLINES {
                    break;
                }
                let pts =
                    transform_points(contour.to_polyline(ARC_TOLERANCE_MM), &expanded.transform);
                if pts.len() >= 3 {
                    acc.polylines.push(VectorPolyline {
                        points: pts,
                        stroke_width_mm: 0.05,
                        clear,
                        closed: true,
                        aperture: None,
                    });
                }
            }
        }
        _ => {}
    }
}

fn collect_aperture_summaries(
    aperture_defs: Option<&std::collections::HashMap<i32, Aperture>>,
    flash_by_code: &BTreeMap<i32, usize>,
    stroke_by_code: &BTreeMap<i32, usize>,
) -> Vec<GerberApertureInfo> {
    let Some(defs) = aperture_defs else {
        return Vec::new();
    };
    let mut sorted_codes: Vec<_> = defs.keys().copied().collect();
    sorted_codes.sort_unstable();
    sorted_codes
        .into_iter()
        .filter_map(|d_code| {
            let ap = defs.get(&d_code)?;
            let (kind, dimensions) = describe_aperture(ap);
            Some(GerberApertureInfo {
                d_code,
                kind,
                dimensions,
                flash_count: flash_by_code.get(&d_code).copied().unwrap_or(0),
                stroke_count: stroke_by_code.get(&d_code).copied().unwrap_or(0),
            })
        })
        .collect()
}

fn collect_flat_stats(
    flat: &FlatLayer,
    aperture_defs: Option<&std::collections::HashMap<i32, Aperture>>,
    corners_lo: lib_gerber_edit::Pos,
    corners_hi: lib_gerber_edit::Pos,
    cancelled: &AtomicBool,
) -> Result<FlatStats, String> {
    let mut acc = FlatAccum {
        flash_count: 0,
        stroke_count: 0,
        region_count: 0,
        clear_count: 0,
        arc_count: 0,
        flash_by_code: BTreeMap::new(),
        stroke_by_code: BTreeMap::new(),
        polylines: Vec::new(),
    };

    let mut aperture_bounds = BTreeMap::new();
    for (&code, image) in &flat.apertures {
        let mut bounds = None;
        for &[x, y] in image.shapes.iter().flatten().flatten() {
            update_bounds(&mut bounds, x, y);
        }
        if let Some(bounds) = bounds {
            aperture_bounds.insert(code, bounds);
        }
    }
    let mut object_bounds = HashMap::new();
    let mut rendered_bounds = None;
    for (idx, expanded) in flat.iter_expanded().enumerate() {
        if idx >= MAX_EXPANDED_OPS {
            return Err("Gerber object count exceeds the preview limit".into());
        }
        if idx.is_multiple_of(512) {
            check_cancelled(cancelled, "Gerber analysis cancelled")?;
        }
        let bounds = object_bounds
            .entry(std::ptr::from_ref(expanded.object))
            .or_insert_with(|| local_object_bounds(expanded.object, &aperture_bounds));
        if let Some([x0, y0, x1, y1]) = *bounds {
            for point in [[x0, y0], [x0, y1], [x1, y0], [x1, y1]] {
                let [x, y] = expanded.transform.apply(point);
                update_bounds(&mut rendered_bounds, x, y);
            }
        }
        record_flat_shape(&mut acc, flat, &expanded);
    }

    let bounds_mm = rendered_bounds.unwrap_or_else(|| {
        if [corners_lo.x, corners_lo.y, corners_hi.x, corners_hi.y]
            .iter()
            .all(|n| n.is_finite())
            && corners_lo.x <= corners_hi.x
            && corners_lo.y <= corners_hi.y
        {
            [corners_lo.x, corners_lo.y, corners_hi.x, corners_hi.y]
        } else {
            [0.0, 0.0, 10.0, 10.0]
        }
    });

    let apertures =
        collect_aperture_summaries(aperture_defs, &acc.flash_by_code, &acc.stroke_by_code);

    Ok(FlatStats {
        apertures,
        flash_count: acc.flash_count,
        stroke_count: acc.stroke_count,
        region_count: acc.region_count,
        clear_count: acc.clear_count,
        arc_count: acc.arc_count,
        bounds_mm,
        polylines: acc.polylines,
    })
}

fn push_flash_polylines(
    out: &mut Vec<VectorPolyline>,
    shapes: &[Vec<Vec<Point>>],
    xf: &Affine2,
    clear: bool,
    aperture: Option<i32>,
) {
    for outer in shapes.iter().filter_map(|shape| shape.first()) {
        if out.len() >= MAX_OVERLAY_POLYLINES {
            break;
        }
        if outer.len() >= 3 {
            out.push(VectorPolyline {
                points: outer.iter().map(|&p| xf.apply(p)).collect(),
                stroke_width_mm: 0.05,
                clear,
                closed: true,
                aperture,
            });
        }
    }
}

fn describe_aperture(ap: &Aperture) -> (String, String) {
    match ap {
        Aperture::Circle(c) => (
            "Circle".into(),
            match c.hole_diameter {
                Some(h) => format!("Ø {:.3} mm (hole {:.3} mm)", c.diameter, h),
                None => format!("Ø {:.3} mm", c.diameter),
            },
        ),
        Aperture::Rectangle(r) => ("Rectangle".into(), format!("{:.3} × {:.3} mm", r.x, r.y)),
        Aperture::Obround(r) => ("Obround".into(), format!("{:.3} × {:.3} mm", r.x, r.y)),
        Aperture::Polygon(p) => (
            "Polygon".into(),
            format!("Ø {:.3} mm, {}v", p.diameter, p.vertices),
        ),
        Aperture::Macro(name, _) => ("Macro".into(), name.clone()),
    }
}

fn aperture_stroke_width(ap: &Aperture) -> f64 {
    match ap {
        Aperture::Circle(c) => c.diameter,
        Aperture::Rectangle(r) | Aperture::Obround(r) => r.x.min(r.y),
        Aperture::Polygon(p) => p.diameter,
        Aperture::Macro(..) => 0.1,
    }
}

fn layer_color_for_type(ty: &LayerType) -> [u8; 3] {
    match ty {
        LayerType::Top => [212, 88, 66],
        LayerType::Bottom => [66, 132, 222],
        LayerType::Inner(idx) => {
            let palette = [
                [218, 168, 66],
                [84, 198, 172],
                [176, 128, 224],
                [148, 206, 94],
            ];
            palette[(*idx as usize) % palette.len()]
        }
        LayerType::MaskTop | LayerType::MaskBottom => [56, 174, 102],
        LayerType::SilkScreenTop | LayerType::SilkScreenBottom => [236, 238, 240],
        LayerType::PasteTop | LayerType::PasteBottom => [172, 180, 194],
        LayerType::Dimensions | LayerType::Milling | LayerType::VCut | LayerType::KeepOut => {
            [240, 202, 68]
        }
        LayerType::CourtyardTop | LayerType::CourtyardBottom => [206, 132, 222],
        LayerType::Drill => DRILL_PALETTE[0],
        _ => [164, 148, 224],
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Passes canvas target, layer colors, polarity flags, and cancellation token explicitly"
)]
fn render_flat_objects(
    canvas: &mut Pixmap,
    flat: &FlatLayer,
    base_rgb: [u8; 3],
    color_by_tool: bool,
    view: Transform,
    scale: f32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut clear_paint = Paint::default();
    clear_paint.set_color_rgba8(base_rgb[0], base_rgb[1], base_rgb[2], 235);
    clear_paint.blend_mode = BlendMode::Clear;
    clear_paint.anti_alias = true;

    let min_stroke_mm = f64::from(0.85 / scale.max(0.01));

    for (idx, expanded) in flat.iter_expanded().take(MAX_EXPANDED_OPS).enumerate() {
        if idx.is_multiple_of(512) {
            check_cancelled(cancelled, "Gerber render cancelled")?;
        }
        let rgb = if color_by_tool {
            let slot = expanded
                .object
                .aperture
                .map(|c| c.unsigned_abs() as usize)
                .unwrap_or(0);
            DRILL_PALETTE[slot % DRILL_PALETTE.len()]
        } else {
            base_rgb
        };
        let mut dark_paint = Paint::default();
        dark_paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
        dark_paint.anti_alias = true;

        let paint = if expanded.polarity == Polarity::Clear {
            &clear_paint
        } else {
            &dark_paint
        };

        draw_expanded_object(canvas, flat, &expanded, paint, view, min_stroke_mm);
    }
    Ok(())
}

fn draw_expanded_object(
    canvas: &mut Pixmap,
    flat: &FlatLayer,
    object: &ExpandedObject,
    paint: &Paint,
    view: Transform,
    min_stroke_mm: f64,
) {
    let image = object
        .object
        .aperture
        .and_then(|code| flat.apertures.get(&code));
    let xf = object.transform;
    match &object.object.shape {
        Shape::Flash => {
            if let Some(image) = image {
                if image.shapes.is_empty() {
                    draw_fallback_dot(canvas, xf.apply([0.0, 0.0]), min_stroke_mm, paint, view);
                } else {
                    fill_shapes(canvas, &image.shapes, &xf, paint, view, min_stroke_mm);
                }
            }
        }
        Shape::Path(path) => {
            let Some(image) = image else { return };
            let points: Vec<Point> = path
                .to_polyline(ARC_TOLERANCE_MM)
                .into_iter()
                .map(|p| xf.apply(p))
                .collect();
            let (width, round) = match &image.aperture {
                Aperture::Circle(c) => (c.diameter, true),
                Aperture::Rectangle(r) | Aperture::Obround(r) => (r.x.min(r.y), false),
                Aperture::Polygon(p) => (p.diameter, false),
                Aperture::Macro(..) => (0.1, false),
            };
            if !round {
                for p in &points {
                    fill_shapes(
                        canvas,
                        &image.shapes,
                        &Affine2::translate(p[0], p[1]),
                        paint,
                        view,
                        min_stroke_mm,
                    );
                }
            }
            if let Some(skia_path) = build_skia_path(points, false) {
                let stroke = Stroke {
                    width: width.max(min_stroke_mm) as f32,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                };
                canvas.stroke_path(&skia_path, paint, &stroke, view, None);
            }
        }
        Shape::Region { contours } => {
            for contour in contours {
                let points: Vec<Point> = contour
                    .to_polyline(ARC_TOLERANCE_MM)
                    .into_iter()
                    .map(|p| xf.apply(p))
                    .collect();
                if let Some(skia_path) = build_skia_path(points, true) {
                    canvas.fill_path(&skia_path, paint, FillRule::Winding, view, None);
                }
            }
        }
        _ => {}
    }
}

fn draw_fallback_dot(
    canvas: &mut Pixmap,
    center: Point,
    radius_mm: f64,
    paint: &Paint,
    view: Transform,
) {
    let mut pb = PathBuilder::new();
    pb.push_circle(
        center[0] as f32,
        center[1] as f32,
        radius_mm.max(0.05) as f32,
    );
    if let Some(path) = pb.finish() {
        canvas.fill_path(&path, paint, FillRule::Winding, view, None);
    }
}

fn fill_shapes(
    canvas: &mut Pixmap,
    shapes: &[Vec<Vec<Point>>],
    xf: &Affine2,
    paint: &Paint,
    view: Transform,
    min_radius_mm: f64,
) {
    for shape in shapes {
        let mut pb = PathBuilder::new();
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for contour in shape {
            append_contour(
                &mut pb,
                contour.iter().map(|p| {
                    let tp = xf.apply(*p);
                    lo[0] = lo[0].min(tp[0]);
                    lo[1] = lo[1].min(tp[1]);
                    hi[0] = hi[0].max(tp[0]);
                    hi[1] = hi[1].max(tp[1]);
                    tp
                }),
                true,
            );
        }
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        if span.is_finite() && span < min_radius_mm * 0.9 {
            let cx = ((lo[0] + hi[0]) * 0.5) as f32;
            let cy = ((lo[1] + hi[1]) * 0.5) as f32;
            let mut dot = PathBuilder::new();
            dot.push_circle(cx, cy, (min_radius_mm * 0.55) as f32);
            if let Some(path) = dot.finish() {
                canvas.fill_path(&path, paint, FillRule::Winding, view, None);
            }
        } else if let Some(path) = pb.finish() {
            canvas.fill_path(&path, paint, FillRule::Winding, view, None);
        }
    }
}

fn build_skia_path(points: Vec<Point>, close: bool) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    append_contour(&mut pb, points, close);
    pb.finish()
}

fn append_contour(pb: &mut PathBuilder, points: impl IntoIterator<Item = Point>, close: bool) {
    let mut points = points.into_iter();
    let Some(first) = points.next() else {
        return;
    };
    pb.move_to(first[0] as f32, first[1] as f32);
    for p in points {
        pb.line_to(p[0] as f32, p[1] as f32);
    }
    if close {
        pb.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gerber_bounds_include_repeated_flashes() {
        let bytes =
            b"%MOMM*%\n%FSLAX26Y26*%\n%ADD10C,2*%\nD10*\n%SRX3Y1I20J0*%\nX0Y0D03*\n%SR*%\nM02*\n";
        let doc =
            GerberDocument::parse_gerber(Path::new("repeated.gbr"), bytes, &AtomicBool::new(false))
                .unwrap();
        assert_eq!(doc.flash_count, 3);
        assert!((doc.bounds_mm[0] + 1.0).abs() < 1e-9);
        assert!((doc.bounds_mm[2] - 41.0).abs() < 1e-9);
    }

    #[test]
    fn x2_file_polarity_preserves_the_layer_image() {
        let picture = "%MOMM*%\n%FSLAX26Y26*%\n%ADD10C,2*%\nD10*\nX0Y0D03*\nM02*\n";
        let parse = |polarity| {
            let bytes = format!("%TF.FilePolarity,{polarity}*%\n{picture}");
            GerberDocument::parse_gerber(
                Path::new("mask.gbr"),
                bytes.as_bytes(),
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        let positive = parse("Positive");
        let negative = parse("Negative");
        assert_eq!(negative.polarity, "Negative");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            positive
                .render_thumbnail([120, 80], &cancel)
                .unwrap()
                .premultiplied_rgba,
            negative
                .render_thumbnail([120, 80], &cancel)
                .unwrap()
                .premultiplied_rgba,
        );
    }

    #[test]
    fn gerber_x2_attributes_clear_polarity_and_thumbnail_render() {
        let gbr = "\
%TF.FileFunction,Copper,L1,Top,Signal*%
%TF.FilePolarity,Positive*%
%TF.GenerationSoftware,KiCad,Pcbnew,8.0.0*%
G04 Copper thickness: 35 um (1 oz)*
%MOIN*%
%FSLAX46Y46*%
%ADD10C,0.040000*%
%ADD11R,0.100000X0.060000*%
%LPD*%
D10*
X0Y0D02*
X1000000Y0D01*
X1000000Y500000D01*
%LPC*%
D11*
X500000Y250000D03*
%LPD*%
G36*
X0Y500000D02*
X200000Y500000D01*
X200000Y700000D01*
X0Y700000D01*
X0Y500000D01*
G37*
M02*
";
        assert!(looks_like_gerber(gbr.as_bytes()));
        let cancel = AtomicBool::new(false);
        let doc =
            GerberDocument::parse_gerber(Path::new("board-F_Cu.gbr"), gbr.as_bytes(), &cancel)
                .unwrap();
        assert!(!doc.is_excellon);
        assert!(doc.layer_type.contains("Copper Top"));
        assert_eq!(doc.units, "Inches (in)");
        assert!(doc.coordinate_format.starts_with("FSLAX46Y46"));
        assert!(doc.polarity.contains("Positive"));
        assert!(doc.polarity.contains("Clear"));
        assert_eq!(doc.copper_thickness.as_deref(), Some("35 um (1 oz)"));
        assert_eq!(doc.aperture_count, 2);
        assert_eq!(doc.clear_count, 1);
        assert_eq!(doc.region_count, 1);
        let width_mm = doc.bounds_mm[2] - doc.bounds_mm[0];
        assert!((25.0..28.0).contains(&width_mm), "width_mm={width_mm}");

        let thumb = doc.render_thumbnail([160, 100], &cancel).unwrap();
        assert!(thumb.width > 0 && thumb.height > 0);
        let non_bg = thumb
            .premultiplied_rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[..3] != [BG_COLOR[0], BG_COLOR[1], BG_COLOR[2]])
            .count();
        assert!(non_bg > 20, "expected rendered Gerber strokes");
    }

    #[test]
    fn excellon_drill_tools_holes_slots_and_arcs_parse_and_render() {
        let drl = "\
M48
; DRILL FILE
METRIC,TZ
T1C0.800
T2C3.200
%
G90
G05
T1
X10.0Y10.0
X20.0Y10.0
X15.0Y15.0G85X25.0Y15.0
T2
X30.0Y20.0
G00X5.0Y5.0
M15
G02X15.0Y5.0I5.0J0.0
M17
M30
";
        let cancel = AtomicBool::new(false);
        let doc = GerberDocument::parse_excellon(Path::new("board.drl"), drl.as_bytes(), &cancel)
            .unwrap();
        assert!(doc.is_excellon);
        assert_eq!(doc.tools.len(), 2);
        assert_eq!(doc.hole_count, 3);
        assert_eq!(doc.slot_count, 2);
        assert_eq!(doc.arc_count, 1);
        assert_eq!(doc.units, "Millimeters (mm)");
        assert_eq!(doc.coordinate_format, "METRIC,TZ,000.000");

        let thumb = doc.render_thumbnail([160, 100], &cancel).unwrap();
        assert!(
            thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [BG_COLOR[0], BG_COLOR[1], BG_COLOR[2]])
        );
    }

    #[test]
    fn gbr_brush_bytes_are_not_misidentified_as_gerber() {
        let mut fake_gimp = vec![0_u8; 32];
        fake_gimp[0..4].copy_from_slice(&28_u32.to_be_bytes());
        fake_gimp[4..8].copy_from_slice(&2_u32.to_be_bytes());
        fake_gimp[8..12].copy_from_slice(&4_u32.to_be_bytes());
        fake_gimp[12..16].copy_from_slice(&4_u32.to_be_bytes());
        fake_gimp[16..20].copy_from_slice(&1_u32.to_be_bytes());
        fake_gimp[20..24].copy_from_slice(b"GIMP");
        assert!(!super::super::is_gerber_gbr_bytes(&fake_gimp));
    }

    #[test]
    fn excellon_repeat_budget_is_cumulative() {
        let pattern = b"R3X1.0\n;R500X1.0\nR2Y1.0\n";
        let cancel = AtomicBool::new(false);
        assert!(check_excellon_repeat_budget(pattern, 5, &cancel).is_ok());
        assert!(check_excellon_repeat_budget(pattern, 4, &cancel).is_err());
    }

    #[test]
    fn kicad_excellon_park_tool_t0_and_empty_npth_parse() {
        let cancel = AtomicBool::new(false);
        let pth = "M48\nFMAT,2\nMETRIC\nT1C0.300\n%\nG90\nG05\nT1\nX4.9Y33.08\nT0\nM30\n";
        let doc = GerberDocument::parse_excellon(Path::new("pth.drl"), pth.as_bytes(), &cancel)
            .expect("KiCad PTH drill file with T0 park command should parse");
        assert!(doc.is_excellon);
        assert_eq!(doc.hole_count, 1);

        let npth = "M48\nFMAT,2\nMETRIC\n%\nG90\nG05\nT0\nM30\n";
        let empty = GerberDocument::parse_excellon(Path::new("npth.drl"), npth.as_bytes(), &cancel)
            .expect("KiCad NPTH drill file with zero holes and T0 should parse");
        assert!(empty.is_excellon);
        assert_eq!(empty.hole_count, 0);
        assert!(empty.render_thumbnail([120, 80], &cancel).is_ok());
    }
}
