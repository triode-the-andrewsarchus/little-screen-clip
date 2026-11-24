//! KiCad `.kicad_pro` project settings and `.gbrjob` Gerber fabrication job specifications.

use super::super::{StripSummary, bounded_source_text, check_cancelled, pixmap_to_thumbnail};
use crate::ImageThumbnail;
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

const PRO_BG_RGBA: [u8; 4] = [20, 24, 30, 255];

fn pro_bg() -> Color {
    Color::from_rgba8(
        PRO_BG_RGBA[0],
        PRO_BG_RGBA[1],
        PRO_BG_RGBA[2],
        PRO_BG_RGBA[3],
    )
}

#[derive(Clone, Debug)]
pub struct KicadNetClassInfo {
    pub name: String,
    pub clearance_mm: Option<f64>,
    pub track_width_mm: Option<f64>,
    pub via_diameter_mm: Option<f64>,
    pub via_drill_mm: Option<f64>,
}

/// Decoded `.kicad_pro` project configuration document.
#[derive(Clone, Debug)]
pub struct KicadProjectDocument {
    pub project_name: String,
    pub meta_version: Option<i64>,
    pub min_track_width_mm: Option<f64>,
    pub min_clearance_mm: Option<f64>,
    pub min_via_diameter_mm: Option<f64>,
    pub copper_edge_clearance_mm: Option<f64>,
    pub net_classes: Vec<KicadNetClassInfo>,
    pub text_vars: Vec<(String, String)>,
    pub sheets: Vec<String>,
    pub pinned_symbol_libs: usize,
    pub pinned_footprint_libs: usize,
    pub design_rules: Vec<(String, String)>,
    pub source_excerpt: String,
}

impl KicadProjectDocument {
    pub fn parse(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "KiCad project parse cancelled")?;
        let root: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid .kicad_pro JSON: {e}"))?;
        let fallback_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();

        let project_name = root
            .pointer("/meta/filename")
            .and_then(Value::as_str)
            .map(|s| s.trim_end_matches(".kicad_pro").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback_name);
        let meta_version = root.pointer("/meta/version").and_then(Value::as_i64);

        let rules_obj = root.pointer("/board/design_settings/rules");
        let design_rules = collect_key_values(rules_obj);
        let net_classes = collect_net_classes(root.pointer("/net_settings/classes"));
        let text_vars = collect_key_values(root.get("text_variables"));
        let sheets = collect_project_sheets(root.get("sheets"));

        let pinned_symbol_libs = root
            .pointer("/libraries/pinned_symbol_libs")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let pinned_footprint_libs = root
            .pointer("/libraries/pinned_footprint_libs")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);

        Ok(Self {
            project_name,
            meta_version,
            min_track_width_mm: rule_f64(rules_obj, "min_track_width"),
            min_clearance_mm: rule_f64(rules_obj, "min_clearance"),
            min_via_diameter_mm: rule_f64(rules_obj, "min_via_diameter"),
            copper_edge_clearance_mm: rule_f64(rules_obj, "copper_edge_clearance"),
            net_classes,
            text_vars,
            sheets,
            pinned_symbol_libs,
            pinned_footprint_libs,
            design_rules,
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        format!(
            "KiCad Project {} · {} net classes · {} sheets · {} rules",
            self.project_name,
            self.net_classes.len(),
            self.sheets.len(),
            self.design_rules.len()
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let track = self
            .min_track_width_mm
            .map_or_else(|| "—".into(), |v| format!("{v:.3} mm"));
        let via = self
            .min_via_diameter_mm
            .map_or_else(|| "—".into(), |v| format!("{v:.3} mm"));
        StripSummary {
            format: super::super::EdaFormat::KicadProject,
            headline: format!("KiCad Project · {}", self.project_name),
            specs: format!(
                "{} net classes · min track {track} · min via {via} · {} vars",
                self.net_classes.len(),
                self.text_vars.len()
            ),
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "KiCad project render cancelled")?;
        let w = max_size[0].clamp(64, 512);
        let h = max_size[1].clamp(48, 512);
        let mut pixmap =
            Pixmap::new(w, h).ok_or_else(|| "Could not allocate project thumbnail".to_string())?;
        pixmap.fill(pro_bg());

        // Draw schematic-style PCB trace & pad design-rule motif proportional to net classes.
        let bars = self.net_classes.len().clamp(1, 5);
        let pad_x = w as f32 * 0.14;
        let usable_w = w as f32 * 0.72;
        let row_h = (h as f32 * 0.66) / bars as f32;
        let top_y = h as f32 * 0.18;

        for idx in 0..bars {
            let cy = top_y + (idx as f32 + 0.5) * row_h;
            let track_px = ((idx + 1) as f32 * 1.4).min(row_h * 0.45);
            let mut paint = Paint::default();
            let palette = [
                [226, 78, 72],
                [158, 206, 106],
                [122, 162, 247],
                [224, 175, 104],
                [125, 207, 255],
            ];
            let [r, g, b] = palette[idx % palette.len()];
            paint.set_color_rgba8(r, g, b, 235);
            paint.anti_alias = true;

            let mut pb = PathBuilder::new();
            pb.move_to(pad_x, cy);
            pb.line_to(pad_x + usable_w * 0.65, cy);
            pb.line_to(pad_x + usable_w * 0.85, cy - row_h * 0.22);
            pb.line_to(pad_x + usable_w, cy - row_h * 0.22);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &Stroke {
                        width: track_px,
                        ..Stroke::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
            let via_r = (track_px * 1.35).clamp(2.5, row_h * 0.35);
            let mut cpb = PathBuilder::new();
            cpb.push_circle(pad_x, cy, via_r);
            cpb.push_circle(pad_x + usable_w, cy - row_h * 0.22, via_r);
            if let Some(cpath) = cpb.finish() {
                pixmap.fill_path(
                    &cpath,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        Ok(pixmap_to_thumbnail(pixmap))
    }
}

#[derive(Clone, Debug)]
pub struct GerberJobFileAttr {
    pub path: String,
    pub file_function: String,
    pub file_polarity: String,
}

#[derive(Clone, Debug)]
pub struct GerberJobStackupLayer {
    pub layer_type: String,
    pub name: String,
    pub color: String,
    pub thickness_mm: Option<f64>,
    pub material: String,
    pub epsilon_r: Option<f64>,
    pub loss_tangent: Option<f64>,
}

/// Decoded KiCad `.gbrjob` manufacturing job specification.
#[derive(Clone, Debug)]
pub struct GerberJobDocument {
    pub project_name: String,
    pub revision: String,
    pub generator: String,
    pub creation_date: String,
    pub board_size_mm: Option<[f64; 2]>,
    pub layer_count: Option<u64>,
    pub board_thickness_mm: Option<f64>,
    pub finish: String,
    pub design_rules: Vec<(String, String)>,
    pub files: Vec<GerberJobFileAttr>,
    pub stackup: Vec<GerberJobStackupLayer>,
    pub source_excerpt: String,
}

impl GerberJobDocument {
    pub fn parse(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "Gerber Job parse cancelled")?;
        let root: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid .gbrjob JSON: {e}"))?;
        let fallback_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("PCB")
            .to_string();

        let project_name = root
            .pointer("/GeneralSpecs/ProjectId/Name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map_or(fallback_name, str::to_owned);
        let revision = root
            .pointer("/GeneralSpecs/ProjectId/Revision")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let generator = format_gbrjob_generator(root.pointer("/Header/GenerationSoftware"));
        let creation_date = root
            .pointer("/Header/CreationDate")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let size_x = root.pointer("/GeneralSpecs/Size/X").and_then(Value::as_f64);
        let size_y = root.pointer("/GeneralSpecs/Size/Y").and_then(Value::as_f64);
        let board_size_mm = match (size_x, size_y) {
            (Some(x), Some(y)) => Some([x, y]),
            _ => None,
        };
        let layer_count = root
            .pointer("/GeneralSpecs/LayerNumber")
            .and_then(Value::as_u64);
        let board_thickness_mm = root
            .pointer("/GeneralSpecs/BoardThickness")
            .and_then(Value::as_f64);
        let finish = root
            .pointer("/GeneralSpecs/Finish")
            .and_then(Value::as_str)
            .unwrap_or("Unspecified")
            .to_string();

        let design_rules = root
            .pointer("/DesignRules/0")
            .map_or_else(Vec::new, |v| collect_key_values(Some(v)));
        let files = collect_gbrjob_files(root.get("FilesAttributes"));
        let stackup = collect_gbrjob_stackup(root.get("MaterialStackup"));

        Ok(Self {
            project_name,
            revision,
            generator,
            creation_date,
            board_size_mm,
            layer_count,
            board_thickness_mm,
            finish,
            design_rules,
            files,
            stackup,
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        let size = self
            .board_size_mm
            .map_or_else(|| "—".into(), |[x, y]| format!("{x:.1}×{y:.1} mm"));
        let layers = self.layer_count.unwrap_or(self.stackup.len() as u64);
        format!(
            "Gerber Job {} · {size} · {layers}L · {} · {} files",
            self.project_name,
            self.finish,
            self.files.len()
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let size = self
            .board_size_mm
            .map_or_else(|| "—".into(), |[x, y]| format!("{x:.1}×{y:.1} mm"));
        let thick = self
            .board_thickness_mm
            .map_or_else(String::new, |t| format!(" · {t:.2} mm"));
        StripSummary {
            format: super::super::EdaFormat::GerberJob,
            headline: format!("Gerber Job · {} · {size}{thick}", self.project_name),
            specs: format!(
                "{} stackup bands · Finish: {} · {} Gerber files",
                self.stackup.len(),
                self.finish,
                self.files.len()
            ),
            detail: self.summary_line(),
        }
    }

    /// Renders a visual PCB stackup cross-section diagram.
    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "Gerber Job render cancelled")?;
        let w = max_size[0].clamp(64, 1024);
        let h = max_size[1].clamp(48, 1024);
        let mut pixmap =
            Pixmap::new(w, h).ok_or_else(|| "Could not allocate stackup preview".to_string())?;
        pixmap.fill(pro_bg());

        if self.stackup.is_empty() {
            return Ok(pixmap_to_thumbnail(pixmap));
        }
        let weights: Vec<f64> = self.stackup.iter().map(stackup_visual_weight).collect();
        let total: f64 = weights.iter().sum::<f64>().max(1.0);
        let margin_x = w as f32 * 0.08;
        let margin_y = h as f32 * 0.10;
        let band_w = (w as f32 - 2.0 * margin_x).max(8.0);
        let avail_h = (h as f32 - 2.0 * margin_y).max(8.0);

        let mut y = margin_y;
        for (layer, weight) in self.stackup.iter().zip(weights.iter()) {
            let bh = ((avail_h as f64) * (*weight / total)) as f32;
            let [r, g, b, a] = stackup_layer_rgba(layer);
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, b, a);
            if let Some(rect) = Rect::from_xywh(margin_x, y, band_w, (bh - 1.0).max(1.5)) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
            y += bh;
        }
        Ok(pixmap_to_thumbnail(pixmap))
    }
}

/// Visual color for a KiCad stackup cross-section band.
pub fn stackup_layer_rgba(layer: &GerberJobStackupLayer) -> [u8; 4] {
    let ty = layer.layer_type.to_ascii_lowercase();
    let col = layer.color.to_ascii_lowercase();
    if ty.contains("copper") {
        return [218, 148, 62, 245];
    }
    if ty.contains("silk") || ty.contains("legend") {
        return if col.contains("black") {
            [45, 48, 54, 240]
        } else {
            [236, 240, 244, 235]
        };
    }
    if ty.contains("mask") {
        return solder_mask_color(&col);
    }
    if ty.contains("dielectric") || ty.contains("core") || ty.contains("prepreg") {
        return [142, 134, 78, 220];
    }
    [120, 136, 152, 200]
}

fn solder_mask_color(col: &str) -> [u8; 4] {
    if col.contains("red") {
        [182, 46, 52, 230]
    } else if col.contains("blue") {
        [44, 96, 178, 230]
    } else if col.contains("black") {
        [40, 44, 52, 235]
    } else if col.contains("white") {
        [222, 226, 230, 235]
    } else if col.contains("purple") {
        [124, 62, 168, 230]
    } else if col.contains("yellow") {
        [204, 168, 48, 230]
    } else {
        [36, 138, 76, 230]
    }
}

fn stackup_visual_weight(layer: &GerberJobStackupLayer) -> f64 {
    let ty = layer.layer_type.to_ascii_lowercase();
    if let Some(t) = layer.thickness_mm {
        if ty.contains("dielectric") {
            return t.clamp(0.08, 0.85);
        }
        return (t * 2.2).clamp(0.05, 0.25);
    }
    if ty.contains("dielectric") {
        0.35
    } else if ty.contains("copper") {
        0.09
    } else {
        0.06
    }
}

fn rule_f64(obj: Option<&Value>, key: &str) -> Option<f64> {
    obj?.get(key)?.as_f64()
}

fn collect_key_values(obj: Option<&Value>) -> Vec<(String, String)> {
    let Some(map) = obj.and_then(Value::as_object) else {
        return Vec::new();
    };
    map.iter()
        .take(64)
        .map(|(k, v)| (k.clone(), format_json_scalar(v)))
        .collect()
}

fn format_json_scalar(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn collect_net_classes(arr: Option<&Value>) -> Vec<KicadNetClassInfo> {
    let Some(items) = arr.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .take(128)
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            Some(KicadNetClassInfo {
                name,
                clearance_mm: item.get("clearance").and_then(Value::as_f64),
                track_width_mm: item.get("track_width").and_then(Value::as_f64),
                via_diameter_mm: item.get("via_diameter").and_then(Value::as_f64),
                via_drill_mm: item.get("via_drill").and_then(Value::as_f64),
            })
        })
        .collect()
}

fn collect_project_sheets(val: Option<&Value>) -> Vec<String> {
    let Some(items) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|pair| {
            pair.as_array()
                .and_then(|arr| arr.get(1))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn format_gbrjob_generator(val: Option<&Value>) -> String {
    let Some(obj) = val.and_then(Value::as_object) else {
        return String::new();
    };
    let app = obj.get("Application").and_then(Value::as_str).unwrap_or("");
    let ver = obj.get("Version").and_then(Value::as_str).unwrap_or("");
    match (app.is_empty(), ver.is_empty()) {
        (false, false) => format!("{app} {ver}"),
        (false, true) => app.to_string(),
        _ => String::new(),
    }
}

fn collect_gbrjob_files(val: Option<&Value>) -> Vec<GerberJobFileAttr> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .take(128)
        .map(|entry| GerberJobFileAttr {
            path: entry
                .get("Path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            file_function: entry
                .get("FileFunction")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            file_polarity: entry
                .get("FilePolarity")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
        .collect()
}

fn collect_gbrjob_stackup(val: Option<&Value>) -> Vec<GerberJobStackupLayer> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .take(128)
        .map(|entry| GerberJobStackupLayer {
            layer_type: entry
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: entry
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            color: entry
                .get("Color")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            thickness_mm: entry.get("Thickness").and_then(Value::as_f64),
            material: entry
                .get("Material")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            epsilon_r: entry.get("EpsilonR").and_then(Value::as_f64),
            loss_tangent: entry.get("LossTangent").and_then(Value::as_f64),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kicad_project_and_gbrjob_extract_rules_stackup_and_render() {
        let pro = r#"{
          "meta": {"filename": "controller.kicad_pro", "version": 1},
          "board": {
            "design_settings": {
              "rules": {
                "min_track_width": 0.15,
                "min_clearance": 0.15,
                "min_via_diameter": 0.45,
                "copper_edge_clearance": 0.3
              }
            }
          },
          "net_settings": {
            "classes": [
              {"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.6, "via_drill": 0.3},
              {"name": "Power", "clearance": 0.25, "track_width": 0.6, "via_diameter": 0.8, "via_drill": 0.4}
            ]
          },
          "text_variables": {"REV": "1.2"},
          "sheets": [["uuid-1", "Root"], ["uuid-2", "Supply"]],
          "libraries": {"pinned_symbol_libs": ["Device"], "pinned_footprint_libs": ["Resistor_SMD"]}
        }"#;
        let cancel = AtomicBool::new(false);
        let pro_doc =
            KicadProjectDocument::parse(Path::new("controller.kicad_pro"), pro.as_bytes(), &cancel)
                .unwrap();
        assert_eq!(pro_doc.project_name, "controller");
        assert_eq!(pro_doc.net_classes.len(), 2);
        assert_eq!(pro_doc.min_track_width_mm, Some(0.15));
        assert_eq!(pro_doc.sheets, vec!["Root", "Supply"]);
        let pro_thumb = pro_doc.render_thumbnail([160, 100], &cancel).unwrap();
        assert!(
            pro_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [PRO_BG_RGBA[0], PRO_BG_RGBA[1], PRO_BG_RGBA[2]])
        );

        let job = r#"{
          "Header": {"GenerationSoftware": {"Application": "Pcbnew", "Version": "8.0.0"}, "CreationDate": "2026-01-01T00:00:00"},
          "GeneralSpecs": {
            "ProjectId": {"Name": "controller", "Revision": "B"},
            "Size": {"X": 80.5, "Y": 42.0},
            "LayerNumber": 4,
            "BoardThickness": 1.6,
            "Finish": "ENIG-RoHS"
          },
          "DesignRules": [{"Zones": "Outer Layers", "PadToPad": 0.15, "MinLineWidth": 0.15}],
          "FilesAttributes": [
            {"Path": "controller-F_Cu.gbr", "FileFunction": "Copper,L1,Top", "FilePolarity": "Positive"},
            {"Path": "controller-B_Cu.gbr", "FileFunction": "Copper,L4,Bot", "FilePolarity": "Positive"}
          ],
          "MaterialStackup": [
            {"Type": "Top Silk Screen", "Name": "Top Silk Screen", "Color": "White"},
            {"Type": "Top Solder Mask", "Name": "Top Solder Mask", "Color": "Green", "Thickness": 0.015},
            {"Type": "Copper", "Name": "F.Cu", "Thickness": 0.035},
            {"Type": "Dielectric", "Name": "Core", "Material": "FR4", "Thickness": 1.4, "EpsilonR": 4.5},
            {"Type": "Copper", "Name": "B.Cu", "Thickness": 0.035}
          ]
        }"#;
        let job_doc =
            GerberJobDocument::parse(Path::new("controller.gbrjob"), job.as_bytes(), &cancel)
                .unwrap();
        assert_eq!(job_doc.board_size_mm, Some([80.5, 42.0]));
        assert_eq!(job_doc.layer_count, Some(4));
        assert_eq!(job_doc.finish, "ENIG-RoHS");
        assert_eq!(job_doc.files.len(), 2);
        assert_eq!(job_doc.stackup.len(), 5);
        let job_thumb = job_doc.render_thumbnail([160, 100], &cancel).unwrap();
        assert!(
            job_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [PRO_BG_RGBA[0], PRO_BG_RGBA[1], PRO_BG_RGBA[2]])
        );
    }
}
