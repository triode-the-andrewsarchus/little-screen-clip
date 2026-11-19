//! Unified brush metadata extraction and cohesive stroke/tip rendering.
//!
//! Supports `.abr`, `.brush`, `.brushset`, `.kpp`, `.bundle`, `.sut`, `.gbr`,
//! `.gih`, `.vbr`, `.mpyb`/`.myb`, and best-effort `.afbrushes`.

mod affinity;
mod csp;
mod gimp;
mod krita;
mod mask;
mod mypaint;
pub mod render;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_BRUSH_FILE_BYTES: usize = 128 * 1024 * 1024;
const MAX_BRUSH_LABEL_BYTES: usize = 512;

fn bounded_label(label: &str) -> &str {
    &label[..label.floor_char_boundary(MAX_BRUSH_LABEL_BYTES)]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushFormat {
    Abr,
    ProcreateBrush,
    ProcreateBrushset,
    KritaPreset,
    KritaBundle,
    ClipStudioSut,
    GimpGbr,
    GimpGih,
    GimpVbr,
    MyPaint,
    AffinityBrushes,
}

impl BrushFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "abr" => Some(Self::Abr),
            "brush" => Some(Self::ProcreateBrush),
            "brushset" => Some(Self::ProcreateBrushset),
            "kpp" => Some(Self::KritaPreset),
            "bundle" => Some(Self::KritaBundle),
            "sut" => Some(Self::ClipStudioSut),
            "gbr" => Some(Self::GimpGbr),
            "gih" => Some(Self::GimpGih),
            "vbr" => Some(Self::GimpVbr),
            "mpyb" | "myb" => Some(Self::MyPaint),
            "afbrushes" => Some(Self::AffinityBrushes),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Abr => "ABR",
            Self::ProcreateBrush => "BRUSH",
            Self::ProcreateBrushset => "BRUSHSET",
            Self::KritaPreset => "KPP",
            Self::KritaBundle => "BUNDLE",
            Self::ClipStudioSut => "SUT",
            Self::GimpGbr => "GBR",
            Self::GimpGih => "GIH",
            Self::GimpVbr => "VBR",
            Self::MyPaint => "MYPAINT",
            Self::AffinityBrushes => "AFBRUSHES",
        }
    }
}

pub fn is_brush_file(path: &Path) -> bool {
    BrushFormat::from_path(path).is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParametricShape {
    Circle,
    Square,
    Diamond,
}

#[derive(Clone, Debug)]
pub enum BrushTipKind {
    /// Grayscale coverage mask (`255` = full ink, `0` = transparent), row-major.
    AlphaMask {
        width: u32,
        height: u32,
        alpha: Vec<u8>,
    },
    /// Parametric dab shape.
    Parametric {
        shape: ParametricShape,
        hardness: f32,
        aspect_ratio: f32,
        angle_deg: f32,
        spikes: u8,
    },
}

#[derive(Clone, Debug)]
pub struct BrushStrokeParams {
    pub size_px: f32,
    pub spacing: f32,
    pub opacity: f32,
    pub angle_deg: f32,
    pub scatter: f32,
    pub hardness: f32,
    pub flow: f32,
}

impl Default for BrushStrokeParams {
    fn default() -> Self {
        Self {
            size_px: 24.0,
            spacing: 0.16,
            opacity: 1.0,
            angle_deg: 0.0,
            scatter: 0.0,
            hardness: 0.85,
            flow: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BrushPreset {
    pub name: String,
    pub tip: BrushTipKind,
    pub params: BrushStrokeParams,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct BrushLibrary {
    pub format: BrushFormat,
    pub brushes: Vec<BrushPreset>,
    pub total_count: usize,
}

/// Loads up to `max_brushes` presets from a supported brush file.
pub fn load_brush_library(
    path: &Path,
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    let (format, bytes) = read_brush_snapshot(path, cancelled)?;
    let fallback_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Brush")
        .to_owned();

    load_brush_bytes(format, &bytes, &fallback_name, max_brushes, cancelled)
}

/// Counts the brushes represented by a supported file.
///
/// Photoshop and Procreate packs use their index. Affinity counts are based on
/// recognizable embedded tip headers. Other formats reuse the first-preset
/// parser and its reported library count.
///
/// # Errors
/// Returns an error for an unreadable or unsupported file, an unrecognized
/// brush library, or a cancelled request.
pub fn count_brushes(path: &Path, cancelled: &AtomicBool) -> Result<usize, String> {
    let (format, bytes) = read_brush_snapshot(path, cancelled)?;
    let count = match format {
        BrushFormat::Abr | BrushFormat::ProcreateBrushset => brushkit_count(format, &bytes)?,
        BrushFormat::AffinityBrushes => affinity::count(&bytes, cancelled)?,
        _ => load_brush_bytes(format, &bytes, "Brush", 1, cancelled)?.total_count,
    };
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    Ok(count)
}

fn read_brush_snapshot(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<(BrushFormat, Vec<u8>), String> {
    let format =
        BrushFormat::from_path(path).ok_or_else(|| "Unsupported brush format".to_owned())?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let bytes = crate::file_source::read_file_snapshot(path, MAX_BRUSH_FILE_BYTES)
        .map_err(|error| format!("Cannot read brush file: {error}"))?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    Ok((format, bytes))
}

/// Parses brush presets from in-memory bytes.
pub fn load_brush_bytes(
    format: BrushFormat,
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let max_brushes = max_brushes.clamp(1, 256);
    match format {
        BrushFormat::Abr | BrushFormat::ProcreateBrush | BrushFormat::ProcreateBrushset => {
            load_via_brushkit(format, bytes, fallback_name, max_brushes, cancelled)
        }
        BrushFormat::KritaPreset => krita::parse_kpp(bytes, fallback_name),
        BrushFormat::KritaBundle => krita::parse_bundle(bytes, max_brushes, cancelled),
        BrushFormat::GimpGbr => gimp::parse_gbr_library(bytes, fallback_name),
        BrushFormat::GimpGih => gimp::parse_gih(bytes, fallback_name, max_brushes),
        BrushFormat::GimpVbr => gimp::parse_vbr(bytes, fallback_name),
        BrushFormat::MyPaint => mypaint::parse_mypaint(bytes, fallback_name),
        BrushFormat::AffinityBrushes => {
            affinity::parse_afbrushes(bytes, fallback_name, max_brushes, cancelled)
        }
        BrushFormat::ClipStudioSut => csp::parse_sut(bytes, fallback_name, cancelled),
    }
}

fn load_via_brushkit(
    format: BrushFormat,
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    let set = brushkit_preview(
        format,
        bytes,
        brushkit::preview::Take::FirstAvailable(max_brushes),
        &mut || !cancelled.load(Ordering::Relaxed),
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }

    let mut brushes = Vec::with_capacity(set.entries.len());
    for entry in set.entries {
        if let brushkit::preview::TipPreview::Available(bitmap) = entry.tip {
            let name = if entry.name.trim().is_empty() {
                set.set_name
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| bounded_label(&s).to_owned())
                    .unwrap_or_else(|| format!("{fallback_name} #{}", entry.index + 1))
            } else {
                bounded_label(&entry.name).to_owned()
            };
            let detail = entry.source_dimensions.map_or_else(
                || format!("{}×{} tip", bitmap.width, bitmap.height),
                |dims| format!("{}×{} tip", dims.width(), dims.height()),
            );
            brushes.push(BrushPreset {
                name,
                tip: BrushTipKind::AlphaMask {
                    width: bitmap.width,
                    height: bitmap.height,
                    alpha: bitmap.data,
                },
                params: BrushStrokeParams::default(),
                detail,
            });
        }
    }

    if brushes.is_empty() {
        return Err(format!("No readable brush tips in {}", format.label()));
    }
    let total_count = brushkit_count(format, bytes)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    Ok(BrushLibrary {
        format,
        brushes,
        total_count,
    })
}

fn brushkit_preview(
    format: BrushFormat,
    bytes: &[u8],
    take: brushkit::preview::Take,
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<brushkit::preview::PreviewSet, String> {
    let bk_format = match format {
        BrushFormat::Abr => brushkit::preview::Format::Abr,
        BrushFormat::ProcreateBrush => brushkit::preview::Format::Brush,
        BrushFormat::ProcreateBrushset => brushkit::preview::Format::Brushset,
        _ => return Err("Unsupported brushkit format".into()),
    };
    let opts = brushkit::preview::PreviewOptions {
        max_cell: mask::PREVIEW_EDGE,
    };
    brushkit::preview::preview(bytes, bk_format, opts, take, keep_going)
        .map_err(|error| format!("Invalid {} brush: {error}", format.label()))
}

fn brushkit_count(format: BrushFormat, bytes: &[u8]) -> Result<usize, String> {
    // A gate that stops before the first entry counts the index without decoding
    // tips. FirstAvailable stops at its sample size without counting the rest.
    Ok(brushkit_preview(format, bytes, brushkit::preview::Take::All, &mut || false)?.not_reached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_gbr(name: &str, w: u32, h: u32, spacing: u32, fill: u8) -> Vec<u8> {
        let mut out = Vec::new();
        let name_bytes = name.as_bytes();
        let header_size = 28 + name_bytes.len() as u32 + 1;
        out.extend_from_slice(&header_size.to_be_bytes());
        out.extend_from_slice(&2_u32.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&1_u32.to_be_bytes());
        out.extend_from_slice(b"GIMP");
        out.extend_from_slice(&spacing.to_be_bytes());
        out.extend_from_slice(name_bytes);
        out.push(0);
        out.resize(out.len() + (w * h) as usize, fill);
        out
    }

    fn make_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 220]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn make_kpp(xml: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 8, 8);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.add_itxt_chunk("preset".into(), xml.into()).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[220; 64]).unwrap();
        writer.finish().unwrap();
        bytes
    }

    #[test]
    fn krita_generated_rectangle_and_embedded_tip_follow_saved_preset() {
        use base64::Engine as _;

        let xml = r#"<Preset name="Rectangle" paintopid="paintbrush"><param name="brush_definition"><![CDATA[<Brush type="auto_brush"><MaskGenerator diameter="42" ratio="0.5" hfade="0.25" vfade="0.75" type="rct"/></Brush>]]></param></Preset>"#;
        let cancelled = AtomicBool::new(false);
        let lib = load_brush_bytes(
            BrushFormat::KritaPreset,
            &make_kpp(xml),
            "fallback",
            1,
            &cancelled,
        )
        .unwrap();
        let tip = &lib.brushes[0];
        assert_eq!(tip.params.size_px, 42.0);
        assert!(matches!(
            tip.tip,
            BrushTipKind::Parametric {
                shape: ParametricShape::Square,
                hardness: 0.5,
                aspect_ratio: 2.0,
                ..
            }
        ));

        let encoded =
            base64::engine::general_purpose::STANDARD.encode(make_gbr("Tip", 8, 4, 20, 200));
        let xml = format!(
            r#"<Preset name="Stamp" paintopid="paintbrush"><resources><resource type="brushes" filename="stamp.gbr" md5sum="one">{encoded}</resource></resources><param name="brush_definition"><![CDATA[<Brush type="gbr_brush" filename="renamed.gbr" md5sum="one" scale="2.0"/>]]></param></Preset>"#
        );
        let lib = load_brush_bytes(
            BrushFormat::KritaPreset,
            &make_kpp(&xml),
            "fallback",
            1,
            &cancelled,
        )
        .unwrap();
        let tip = &lib.brushes[0];
        assert_eq!(tip.params.size_px, 16.0);
        assert!(matches!(
            &tip.tip,
            BrushTipKind::AlphaMask { width: 8, height: 4, alpha }
                if alpha == &[200; 32]
        ));
    }

    #[test]
    fn abr_and_procreate_totals_include_brushes_beyond_the_preview_limit() {
        let token = AtomicBool::new(false);
        let mut samples = Vec::new();
        for coverage in [200, 220] {
            let mut sample = vec![0; 4];
            for bound in [0_i32, 0, 4, 4] {
                sample.extend_from_slice(&bound.to_be_bytes());
            }
            sample.extend_from_slice(&[0, 8, 0]);
            sample.extend([coverage; 16]);
            samples.extend_from_slice(&(sample.len() as u32).to_be_bytes());
            samples.extend(sample);
            samples.resize(samples.len().next_multiple_of(4), 0);
        }
        let mut abr = b"\0\x06\0\x028BIMsamp".to_vec();
        abr.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        abr.extend_from_slice(&samples);

        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        let brush_archive = br#"<?xml version="1.0"?><plist version="1.0"><dict><key>$objects</key><array><string>$null</string><dict/></array></dict></plist>"#;
        let shape = make_png(4, 4);
        for prefix in ["a", "b"] {
            archive
                .start_file(format!("{prefix}/Brush.archive"), options)
                .unwrap();
            archive.write_all(brush_archive).unwrap();
            archive
                .start_file(format!("{prefix}/Shape.png"), options)
                .unwrap();
            archive.write_all(&shape).unwrap();
        }
        let brushset = archive.finish().unwrap().into_inner();

        for (format, bytes) in [
            (BrushFormat::Abr, abr),
            (BrushFormat::ProcreateBrushset, brushset),
        ] {
            let library = load_brush_bytes(format, &bytes, "fallback", 1, &token).unwrap();
            assert_eq!(library.brushes.len(), 1);
            assert_eq!(library.total_count, 2);
        }
    }

    #[test]
    fn brushset_count_uses_the_pack_index_even_when_tips_are_missing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("indexed.brushset");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for folder in ["first", "second"] {
            archive
                .start_file(
                    format!("{folder}/Brush.archive"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            archive.write_all(b"metadata-only fixture").unwrap();
        }
        archive.finish().unwrap();
        let cancelled = AtomicBool::new(false);
        assert_eq!(count_brushes(&path, &cancelled).unwrap(), 2);
        assert!(load_brush_library(&path, 1, &cancelled).is_err());
    }

    #[test]
    fn large_brush_tips_keep_a_bounded_preview_and_original_dimensions() {
        let gbr = make_gbr("Wide", 512, 256, 20, 255);
        let lib = load_brush_bytes(
            BrushFormat::GimpGbr,
            &gbr,
            "fallback",
            1,
            &AtomicBool::new(false),
        )
        .unwrap();
        let brush = &lib.brushes[0];
        assert!(matches!(
            &brush.tip,
            BrushTipKind::AlphaMask { width: 192, height: 96, alpha }
                if alpha.len() == 192 * 96
        ));
        assert!(brush.detail.starts_with("512×256"));
    }

    #[test]
    fn gimp_rgba_gbr_uses_alpha_even_for_an_opaque_white_tip() {
        let mut file = make_gbr("White", 4, 4, 20, 255);
        file[16..20].copy_from_slice(&4_u32.to_be_bytes());
        file.extend([255; 48]);
        let lib = load_brush_bytes(
            BrushFormat::GimpGbr,
            &file,
            "fallback",
            1,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(matches!(
            &lib.brushes[0].tip,
            BrushTipKind::AlphaMask { alpha, .. } if alpha == &[255; 16]
        ));
    }

    #[test]
    fn gimp_vbr_1_0_reads_all_fields_without_a_shape_line() {
        let file = b"GIMP-VBR\n1.0\nSoft Round\n30\n28\n0.65\n1.4\n15\n";
        let lib = load_brush_bytes(
            BrushFormat::GimpVbr,
            file,
            "fallback",
            1,
            &AtomicBool::new(false),
        )
        .unwrap();
        let brush = &lib.brushes[0];
        assert_eq!(brush.params.spacing, 0.30);
        assert_eq!(brush.params.size_px, 56.0);
        assert_eq!(brush.params.hardness, 0.65);
        assert_eq!(brush.params.angle_deg, 15.0);
        assert!(matches!(
            brush.tip,
            BrushTipKind::Parametric {
                shape: ParametricShape::Circle,
                aspect_ratio: 1.4,
                spikes: 2,
                ..
            }
        ));
    }

    #[test]
    fn parses_gimp_gbr_gih_and_vbr_and_renders_cohesive_strokes() {
        let cancel = AtomicBool::new(false);
        let gbr = make_gbr("Soft Round", 8, 8, 20, 255);
        let lib = load_brush_bytes(BrushFormat::GimpGbr, &gbr, "fallback", 8, &cancel).unwrap();
        assert_eq!(lib.brushes.len(), 1);
        assert_eq!(lib.brushes[0].name, "Soft Round");
        let thumb = render::render_brush_preview(&lib.brushes[0], 128, 48, [240, 240, 240], true);
        assert!(
            thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0)
        );

        let mut gih = b"Hose\n2 ncells:2\n".to_vec();
        gih.extend_from_slice(&make_gbr("Cell A", 4, 4, 15, 200));
        gih.extend_from_slice(&make_gbr("Cell B", 4, 4, 15, 180));
        let gih_lib = load_brush_bytes(BrushFormat::GimpGih, &gih, "hose", 8, &cancel).unwrap();
        assert_eq!(gih_lib.brushes.len(), 2);

        let vbr = b"GIMP-VBR\n1.5\nStar Tip\ncircle\n15\n24\n5\n0.75\n1.0\n0.0\n";
        let vbr_lib = load_brush_bytes(BrushFormat::GimpVbr, vbr, "star", 8, &cancel).unwrap();
        assert_eq!(vbr_lib.brushes[0].name, "Star Tip");
        let vbr_thumb =
            render::render_brush_preview(&vbr_lib.brushes[0], 96, 36, [255, 255, 255], false);
        assert!(
            vbr_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0)
        );
    }

    #[test]
    fn parses_krita_kpp_and_bundle_and_mypaint_and_afbrushes() {
        let cancel = AtomicBool::new(false);
        let kpp_xml = r#"<Preset name="Ink Pen" paintopid="paintbrush"><param name="Opacity">0.9</param><param name="brush_definition">&lt;Brush type="auto_brush" spacing="0.12"&gt;&lt;MaskGenerator radius="14" fade="0.2" ratio="0.8" spikes="2" type="circle"/&gt;&lt;/Brush&gt;</param></Preset>"#;
        let kpp = make_kpp(kpp_xml);
        let kpp_lib = load_brush_bytes(BrushFormat::KritaPreset, &kpp, "ink", 8, &cancel).unwrap();
        assert_eq!(kpp_lib.brushes[0].name, "Ink Pen");

        let mut bundle_bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bundle_bytes));
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("brushes/tip.gbr", opts).unwrap();
            zip.write_all(&make_gbr("Bundled Tip", 6, 6, 15, 255))
                .unwrap();
            zip.start_file("paintoppresets/ink.kpp", opts).unwrap();
            zip.write_all(&kpp).unwrap();
            zip.finish().unwrap();
        }
        let bundle_lib =
            load_brush_bytes(BrushFormat::KritaBundle, &bundle_bytes, "pack", 8, &cancel).unwrap();
        assert_eq!(bundle_lib.brushes.len(), 2);

        let myb = br#"{"version":3,"parent_brush_name":"Charcoal","settings":{"radius_logarithmic":{"base_value":2.2},"hardness":{"base_value":0.6}}}"#;
        let myb_lib = load_brush_bytes(BrushFormat::MyPaint, myb, "charcoal", 8, &cancel).unwrap();
        assert_eq!(myb_lib.brushes[0].name, "Charcoal");

        let mut af = b"\x00\xffKAtest".to_vec();
        af.extend_from_slice(&make_png(8, 8));
        let af_lib =
            load_brush_bytes(BrushFormat::AffinityBrushes, &af, "affinity", 8, &cancel).unwrap();
        assert_eq!(af_lib.brushes.len(), 1);
    }

    #[test]
    fn parses_clip_studio_sut_sqlite_file() {
        let dir = tempfile::tempdir().unwrap();
        let sut_path = dir.path().join("gpen.sut");
        let conn = rusqlite::Connection::open(&sut_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE Node (NodeName TEXT);
             CREATE TABLE Variant (BrushSize REAL, BrushHardness REAL, BrushInterval REAL, Opacity REAL);
             CREATE TABLE MaterialFile (FileData BLOB);
             INSERT INTO Node (NodeName) VALUES ('G-Pen');
             INSERT INTO Variant (BrushSize, BrushHardness, BrushInterval, Opacity) VALUES (18.0, 90.0, 12.0, 100.0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO MaterialFile (FileData) VALUES (?1)",
            [&make_png(8, 8)],
        )
        .unwrap();
        drop(conn);

        let cancel = AtomicBool::new(false);
        let lib = load_brush_library(&sut_path, 8, &cancel).unwrap();
        assert_eq!(lib.format, BrushFormat::ClipStudioSut);
        assert_eq!(lib.brushes[0].name, "G-Pen");
        assert_eq!(lib.brushes[0].params.size_px, 18.0);

        let snapshot = std::fs::read(&sut_path).unwrap();
        let from_bytes = load_brush_bytes(
            BrushFormat::ClipStudioSut,
            &snapshot,
            "fallback",
            8,
            &cancel,
        )
        .unwrap();
        assert_eq!(from_bytes.brushes[0].name, "G-Pen");
        assert!(
            load_brush_bytes(
                BrushFormat::ClipStudioSut,
                b"not SQLite",
                "fallback",
                8,
                &cancel
            )
            .is_err()
        );
        assert!(matches!(lib.brushes[0].tip, BrushTipKind::AlphaMask { .. }));
    }
}
