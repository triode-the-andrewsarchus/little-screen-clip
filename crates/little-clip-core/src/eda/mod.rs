//! Bounded read-only EDA, PCB fabrication, centroid, and plotter parsers and rasterizers.

pub mod gerber;
pub mod hpgl;
pub mod kicad;
pub mod pick_place;

use crate::ImageThumbnail;
use crate::file_source::read_file_snapshot_with_source;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tiny_skia::{Color, Pixmap, Transform};

/// Maximum source bytes read for EDA documents (32 MiB).
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
/// Maximum source text retained for interactive source inspection in viewers.
pub const MAX_SOURCE_CHARS: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdaFormat {
    Gerber,
    Excellon,
    PickAndPlace,
    Hpgl,
    KicadPcb,
    KicadSchematic,
    KicadProject,
    KicadFootprint,
    KicadSymbolLib,
    GerberJob,
}

const UNIFIED_EXT_MAP: &[(&str, EdaFormat)] = &[
    ("ger", EdaFormat::Gerber),
    ("drl", EdaFormat::Excellon),
    ("xln", EdaFormat::Excellon),
    ("pos", EdaFormat::PickAndPlace),
    ("plt", EdaFormat::Hpgl),
    ("hpgl", EdaFormat::Hpgl),
    ("kicad_pcb", EdaFormat::KicadPcb),
    ("kicad_sch", EdaFormat::KicadSchematic),
    ("kicad_pro", EdaFormat::KicadProject),
    ("kicad_mod", EdaFormat::KicadFootprint),
    ("kicad_sym", EdaFormat::KicadSymbolLib),
    ("gbrjob", EdaFormat::GerberJob),
];

/// Returns whether a filename extension unambiguously identifies an EDA file format.
///
/// `.gbr` is intentionally excluded because it collides with GIMP bitmap brushes.
pub fn supports_extension(ext: &str) -> bool {
    EdaFormat::from_extension_str(ext).is_some()
}

impl EdaFormat {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gerber => "Gerber",
            Self::Excellon => "Excellon Drill",
            Self::PickAndPlace => "Pick & Place",
            Self::Hpgl => "HPGL Plot",
            Self::KicadPcb => "KiCad PCB",
            Self::KicadSchematic => "KiCad Schematic",
            Self::KicadProject => "KiCad Project",
            Self::KicadFootprint => "KiCad Footprint",
            Self::KicadSymbolLib => "KiCad Symbol Library",
            Self::GerberJob => "Gerber Job",
        }
    }

    pub fn from_extension_str(ext: &str) -> Option<Self> {
        UNIFIED_EXT_MAP
            .iter()
            .find(|(candidate, _)| ext.eq_ignore_ascii_case(candidate))
            .map(|(_, format)| *format)
    }

    pub fn extension_hint(path: &Path) -> Option<Self> {
        Self::from_extension_str(path.extension()?.to_str()?)
    }
}

/// Decoded EDA document across fabrication, KiCad, and simulation formats.
#[derive(Debug, Clone)]
pub enum Document {
    Gerber(Box<gerber::GerberDocument>),
    PickAndPlace(pick_place::PickAndPlaceDocument),
    Hpgl(hpgl::HpglDocument),
    KicadPcb(Box<kicad::KicadPcbDocument>),
    KicadSchematic(Box<kicad::KicadSchematicDocument>),
    KicadProject(Box<kicad::KicadProjectDocument>),
    KicadFootprint(Box<kicad::KicadFootprintDocument>),
    KicadSymbolLib(Box<kicad::KicadSymbolLibDocument>),
    GerberJob(Box<kicad::GerberJobDocument>),
}

impl Document {
    pub fn format(&self) -> EdaFormat {
        match self {
            Self::Gerber(doc) if doc.is_excellon => EdaFormat::Excellon,
            Self::Gerber(_) => EdaFormat::Gerber,
            Self::PickAndPlace(_) => EdaFormat::PickAndPlace,
            Self::Hpgl(_) => EdaFormat::Hpgl,
            Self::KicadPcb(_) => EdaFormat::KicadPcb,
            Self::KicadSchematic(_) => EdaFormat::KicadSchematic,
            Self::KicadProject(_) => EdaFormat::KicadProject,
            Self::KicadFootprint(_) => EdaFormat::KicadFootprint,
            Self::KicadSymbolLib(_) => EdaFormat::KicadSymbolLib,
            Self::GerberJob(_) => EdaFormat::GerberJob,
        }
    }

    /// Compact single-line summary for list strips and metadata headers.
    pub fn summary_line(&self) -> String {
        match self {
            Self::Gerber(doc) => doc.summary_line(),
            Self::PickAndPlace(doc) => doc.summary_line(),
            Self::Hpgl(doc) => doc.summary_line(),
            Self::KicadPcb(doc) => doc.summary_line(),
            Self::KicadSchematic(doc) => doc.summary_line(),
            Self::KicadProject(doc) => doc.summary_line(),
            Self::KicadFootprint(doc) => doc.summary_line(),
            Self::KicadSymbolLib(doc) => doc.summary_line(),
            Self::GerberJob(doc) => doc.summary_line(),
        }
    }

    /// Bounded premultiplied RGBA raster thumbnail for cards and strips.
    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        match self {
            Self::Gerber(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::PickAndPlace(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::Hpgl(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::KicadPcb(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::KicadSchematic(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::KicadProject(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::KicadFootprint(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::KicadSymbolLib(doc) => doc.render_thumbnail(max_size, cancelled),
            Self::GerberJob(doc) => doc.render_thumbnail(max_size, cancelled),
        }
    }

    /// Lightweight metadata summary stored in file-list preview caches.
    pub fn strip_summary(&self) -> StripSummary {
        match self {
            Self::Gerber(doc) => gerber_strip_summary(doc, self.format(), self.summary_line()),
            Self::PickAndPlace(doc) => {
                pick_place_strip_summary(doc, self.format(), self.summary_line())
            }
            Self::Hpgl(doc) => hpgl_strip_summary(doc, self.format(), self.summary_line()),
            Self::KicadPcb(doc) => doc.strip_summary(),
            Self::KicadSchematic(doc) => doc.strip_summary(),
            Self::KicadProject(doc) => doc.strip_summary(),
            Self::KicadFootprint(doc) => doc.strip_summary(),
            Self::KicadSymbolLib(doc) => doc.strip_summary(),
            Self::GerberJob(doc) => doc.strip_summary(),
        }
    }
}

fn gerber_strip_summary(
    doc: &gerber::GerberDocument,
    format: EdaFormat,
    detail: String,
) -> StripSummary {
    let w = (doc.bounds_mm[2] - doc.bounds_mm[0]).max(0.0);
    let h = (doc.bounds_mm[3] - doc.bounds_mm[1]).max(0.0);
    if doc.is_excellon {
        return StripSummary {
            format,
            headline: format!(
                "{} · {} tools · {} holes",
                doc.layer_type,
                doc.tools.len(),
                doc.hole_count
            ),
            specs: format!(
                "{} · {} slots · {} arcs · {:.1}×{:.1} mm",
                doc.coordinate_format, doc.slot_count, doc.arc_count, w, h
            ),
            detail,
        };
    }
    let mut parts = vec![
        doc.coordinate_format.clone(),
        doc.units.clone(),
        doc.polarity.clone(),
    ];
    if let Some(cu) = &doc.copper_thickness {
        parts.push(format!("Cu {cu}"));
    }
    parts.push(format!("{} apertures", doc.aperture_count));
    StripSummary {
        format,
        headline: format!("Gerber · {}", doc.layer_type),
        specs: parts.join(" · "),
        detail,
    }
}

fn pick_place_strip_summary(
    doc: &pick_place::PickAndPlaceDocument,
    format: EdaFormat,
    detail: String,
) -> StripSummary {
    let w = (doc.bounds_mm[2] - doc.bounds_mm[0]).max(0.0);
    let h = (doc.bounds_mm[3] - doc.bounds_mm[1]).max(0.0);
    StripSummary {
        format,
        headline: format!(
            "Pick & Place · {} parts ({} Top / {} Bottom)",
            doc.placements.len(),
            doc.top_count,
            doc.bottom_count
        ),
        specs: format!(
            "{} · {} packages · {:.1}×{:.1} mm",
            doc.units, doc.unique_packages, w, h
        ),
        detail,
    }
}

fn hpgl_strip_summary(doc: &hpgl::HpglDocument, format: EdaFormat, detail: String) -> StripSummary {
    let w = (doc.bounds_mm[2] - doc.bounds_mm[0]).max(0.0);
    let h = (doc.bounds_mm[3] - doc.bounds_mm[1]).max(0.0);
    StripSummary {
        format,
        headline: format!(
            "HPGL Plot · {} pens · {} strokes",
            doc.pens_used.len().max(1),
            doc.draw_count
        ),
        specs: format!(
            "Draw {:.1} mm · {} moves · {} labels · {:.1}×{:.1} mm",
            doc.draw_length_mm,
            doc.move_count,
            doc.labels.len(),
            w,
            h
        ),
        detail,
    }
}

/// Compact metadata summary retained for list rows without holding full geometry in memory.
#[derive(Debug, Clone)]
pub struct StripSummary {
    pub format: EdaFormat,
    pub headline: String,
    pub specs: String,
    pub detail: String,
}

/// Returns whether a `.gbr` byte slice is a Gerber fabrication file rather than a GIMP brush.
pub fn is_gerber_gbr_bytes(bytes: &[u8]) -> bool {
    !crate::brushes::is_gimp_gbr_header(bytes) && gerber::looks_like_gerber(bytes)
}

/// Returns whether a `.gbr` path holds a Gerber fabrication file rather than a GIMP brush.
pub fn is_gerber_gbr_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gbr"))
        && crate::file_source::read_file_bytes(path, 8192, false, true)
            .is_ok_and(|bytes| is_gerber_gbr_bytes(&bytes))
}

/// Reads and parses an EDA file through core's guarded `FileSource` snapshot helper.
pub fn read_document(path: &Path, cancelled: &AtomicBool) -> Result<Document, String> {
    check_cancelled(cancelled, "EDA read cancelled")?;
    let (bytes, source) =
        read_file_snapshot_with_source(path, MAX_BYTES).map_err(|error| error.to_string())?;
    check_cancelled(cancelled, "EDA read cancelled")?;
    let doc = parse_bytes(path, &bytes, cancelled)?;
    source.check()?;
    check_cancelled(cancelled, "EDA read cancelled")?;
    Ok(doc)
}

/// Parses an in-memory buffer according to path extension and content disambiguation.
pub fn parse_bytes(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<Document, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "gbr" => {
            if crate::brushes::is_gimp_gbr_header(bytes) {
                return Err("File is a GIMP binary brush, not a Gerber layer".into());
            }
            gerber::GerberDocument::parse_gerber(path, bytes, cancelled)
                .map(|doc| Document::Gerber(Box::new(doc)))
        }
        "ger" => gerber::GerberDocument::parse_gerber(path, bytes, cancelled)
            .map(|doc| Document::Gerber(Box::new(doc))),
        "drl" | "xln" => gerber::GerberDocument::parse_excellon(path, bytes, cancelled)
            .map(|doc| Document::Gerber(Box::new(doc))),
        "pos" => {
            pick_place::PickAndPlaceDocument::parse(bytes, cancelled).map(Document::PickAndPlace)
        }
        "plt" | "hpgl" => hpgl::HpglDocument::parse(bytes, cancelled).map(Document::Hpgl),
        ext @ ("kicad_pcb" | "kicad_mod" | "kicad_sch" | "kicad_sym" | "kicad_pro" | "gbrjob") => {
            kicad::parse_kicad(ext, path, bytes, cancelled)
        }
        _ => Err("Unsupported EDA file format".into()),
    }
}

/// Renders a bounded thumbnail directly from an on-disk ordinary file.
pub fn render_thumbnail(
    path: &Path,
    max_size: [u32; 2],
    cancelled: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    let doc = read_document(path, cancelled)?;
    doc.render_thumbnail(max_size, cancelled)
}

pub(crate) fn check_cancelled(cancelled: &AtomicBool, message: &str) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(message.into());
    }
    Ok(())
}

pub(crate) fn bounded_source_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_SOURCE_CHARS {
        text.into_owned()
    } else {
        let end = text.floor_char_boundary(MAX_SOURCE_CHARS);
        let mut out = String::with_capacity(end + 64);
        out.push_str(&text[..end]);
        out.push_str("\n… [source truncated for preview]");
        out
    }
}

pub(crate) fn allocate_canvas(
    bounds: [f64; 4],
    max_size: [u32; 2],
    bg: Color,
) -> Option<(Pixmap, Transform, f32)> {
    allocate_canvas_with_y_flip(bounds, max_size, bg, false)
}

pub(crate) fn allocate_canvas_with_y_flip(
    bounds: [f64; 4],
    max_size: [u32; 2],
    bg: Color,
    y_down: bool,
) -> Option<(Pixmap, Transform, f32)> {
    let max_w = max_size[0].clamp(16, 2048);
    let max_h = max_size[1].clamp(16, 2048);
    let span_x = (bounds[2] - bounds[0]).abs();
    let span_y = (bounds[3] - bounds[1]).abs();
    let (span_x, span_y) = if span_x < 1e-6 && span_y < 1e-6 {
        (10.0, 10.0)
    } else {
        (span_x.max(0.01), span_y.max(0.01))
    };
    let aspect = span_x / span_y;
    let target_aspect = f64::from(max_w) / f64::from(max_h);
    let (pw, ph) = if aspect >= target_aspect {
        (
            max_w,
            ((f64::from(max_w) / aspect).round() as u32).clamp(16, max_h),
        )
    } else {
        (
            ((f64::from(max_h) * aspect).round() as u32).clamp(16, max_w),
            max_h,
        )
    };
    let mut pixmap = Pixmap::new(pw, ph)?;
    pixmap.fill(bg);
    let pad = (pw.min(ph) as f64 * 0.08).clamp(3.0, 18.0);
    let avail_w = (f64::from(pw) - 2.0 * pad).max(1.0);
    let avail_h = (f64::from(ph) - 2.0 * pad).max(1.0);
    let scale = (avail_w / span_x).min(avail_h / span_y);
    let cx = (bounds[0] + bounds[2]) * 0.5;
    let cy = (bounds[1] + bounds[3]) * 0.5;
    let tx = (f64::from(pw) * 0.5 - cx * scale) as f32;
    let (sy, ty) = if y_down {
        (scale as f32, (f64::from(ph) * 0.5 - cy * scale) as f32)
    } else {
        (-(scale as f32), (f64::from(ph) * 0.5 + cy * scale) as f32)
    };
    let transform = Transform::from_row(scale as f32, 0.0, 0.0, sy, tx, ty);
    Some((pixmap, transform, scale as f32))
}

pub(crate) fn pixmap_to_thumbnail(pixmap: Pixmap) -> ImageThumbnail {
    ImageThumbnail {
        width: pixmap.width(),
        height: pixmap.height(),
        premultiplied_rgba: pixmap.take(),
    }
}

pub(crate) fn update_bounds(bounds: &mut Option<[f64; 4]>, x: f64, y: f64) {
    if !x.is_finite() || !y.is_finite() {
        return;
    }
    match bounds {
        Some(b) => {
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x);
            b[3] = b[3].max(y);
        }
        None => *bounds = Some([x, y, x, y]),
    }
}
