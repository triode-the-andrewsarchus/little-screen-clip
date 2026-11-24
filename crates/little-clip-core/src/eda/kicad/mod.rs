//! Native KiCad EDA parsers and rasterizers (`.kicad_pcb`, `.kicad_sch`, `.kicad_pro`,
//! `.kicad_mod`, `.kicad_sym`, and `.gbrjob`).

pub mod pcb;
pub mod project;
pub mod scene_render;
pub mod schematic;

pub use pcb::{
    KicadFootprintDocument, KicadPcbDocument, PcbFootprintSummary, PcbLayerBucket, PcbLayerInfo,
    PcbNetInfo, PcbPadInfo, PcbPrim,
};
pub use project::{
    GerberJobDocument, GerberJobFileAttr, GerberJobStackupLayer, KicadNetClassInfo,
    KicadProjectDocument,
};
pub use schematic::{
    KicadSchematicDocument, KicadSymbolEntry, KicadSymbolLibDocument, KicadSymbolPinInfo,
    SchematicLabelInfo, SchematicLabelKind, SchematicSheetInfo, SchematicSymbolInfo,
};

use super::Document;
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Parses a KiCad document according to its lowercased extension.
pub fn parse_kicad(
    ext: &str,
    path: &Path,
    bytes: &[u8],
    cancelled: &AtomicBool,
) -> Result<Document, String> {
    match ext {
        "kicad_pcb" => {
            pcb::KicadPcbDocument::parse(bytes, cancelled).map(|d| Document::KicadPcb(Box::new(d)))
        }
        "kicad_mod" => pcb::KicadFootprintDocument::parse(bytes, cancelled)
            .map(|d| Document::KicadFootprint(Box::new(d))),
        "kicad_sch" => schematic::KicadSchematicDocument::parse(path, bytes, cancelled)
            .map(|d| Document::KicadSchematic(Box::new(d))),
        "kicad_sym" => schematic::KicadSymbolLibDocument::parse(bytes, cancelled)
            .map(|d| Document::KicadSymbolLib(Box::new(d))),
        "kicad_pro" => project::KicadProjectDocument::parse(path, bytes, cancelled)
            .map(|d| Document::KicadProject(Box::new(d))),
        "gbrjob" => project::GerberJobDocument::parse(path, bytes, cancelled)
            .map(|d| Document::GerberJob(Box::new(d))),
        _ => Err("Unsupported KiCad EDA format".into()),
    }
}
