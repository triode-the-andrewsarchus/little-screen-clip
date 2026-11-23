//! Cheap format hints shared by strips, row admission and studio dispatch.
//!
//! These hints do not establish content identity, readability or edit support.
//! Workers validate ordinary sources and decode them before publishing prepared
//! documents. Ambiguous sampler names require positive content identification;
//! they retain their ordinary routing until a worker supplies that evidence.
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
/// Cheap semantic-document path hints; workers establish actual content support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Font,
    Notebook,
    Book,
    Mail,
    Calendar,
    Database,
    Geography,
    Toolpath,
    Embroidery,
    Multisample,
    Subtitle,
    Spreadsheet,
    VcvPatch,
    Oscilloscope,
    PowerRecording,
    Touchstone,
    LogicCapture,
    PacketCapture,
    CanCapture,
}
impl Format {
    /// Returns the shared filename hint without reading or admitting the source.
    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        let lower;
        let extension = if extension.bytes().any(|b| b.is_ascii_uppercase()) {
            lower = extension.to_ascii_lowercase();
            lower.as_str()
        } else {
            extension
        };
        match extension {
            "ttf" | "ttc" | "otf" | "otc" | "woff" | "woff2" => Some(Self::Font),
            "ipynb" => Some(Self::Notebook),
            "epub" | "cbz" => Some(Self::Book),
            "eml" => Some(Self::Mail),
            "ics" => Some(Self::Calendar),
            "sqlite" | "sqlite3" | "db" | "db3" => Some(Self::Database),
            "gpx" | "geojson" | "rctrk" => Some(Self::Geography),
            "gcode" => Some(Self::Toolpath),
            "wfm" => Some(Self::Oscilloscope),
            "jls" => Some(Self::PowerRecording),
            "sr" | "sal" | "vcd" => Some(Self::LogicCapture),
            "pcap" | "pcapng" => Some(Self::PacketCapture),
            _ if crate::engineering::touchstone::extension_ports(path).is_some() => {
                Some(Self::Touchstone)
            }
            ext if crate::embroidery::supports_extension(ext) => Some(Self::Embroidery),
            "sfz" | "sf2" | "nki" | "dspreset" | "dslibrary" | "multisample" | "xrni" => {
                Some(Self::Multisample)
            }
            "vcv" | "vcvss" => Some(Self::VcvPatch),
            "csv" | "tsv" | "xlsx" | "xls" | "xlsb" | "ods" => Some(Self::Spreadsheet),
            _ => crate::subtitles::is_subtitle_path(path).then_some(Self::Subtitle),
        }
    }
}
/// Whether an extension requires positive content evidence to override routing.
pub fn ambiguous_engineering_path(path: &Path) -> bool {
    packet_probe_path(path)
        || path.extension().is_some_and(|extension| {
            [
                "csv", "bin", "ts", "dsl", "trace", "asc", "blf", "trc", "log", "candump",
            ]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn packet_probe_path(path: &Path) -> bool {
    crate::engineering::packets::is_candidate_path(path)
        && Format::for_path(path) != Some(Format::PacketCapture)
}

/// Check a bounded, identity-verified prefix on a content worker.
///
/// No hint leaves existing routing in place. The successful decoder must still
/// validate its own pinned input, including any format-specific offsets.
pub fn recognize_engineering(path: &Path, cancel: &AtomicBool) -> Option<Format> {
    use crate::{
        engineering::{can, logic, packets, scope, touchstone},
        file_source::ReadCheck,
    };

    if !ambiguous_engineering_path(path) || cancel.load(Ordering::Relaxed) {
        return None;
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let packet = packet_probe_path(path);
    let can = matches!(
        extension.as_str(),
        "trace" | "asc" | "blf" | "trc" | "log" | "candump"
    );
    let bound = if packet || extension == "ts" {
        64 * 1024
    } else if can {
        can::HEADER_BYTES
    } else {
        scope::HEADER_BYTES
    };
    let check = ReadCheck::default();
    let (file, source) = check.open(path, None, cancel).ok()?;
    let length = file.metadata().ok()?.len();
    let mut bytes = Vec::new();
    (&file).take(bound as u64).read_to_end(&mut bytes).ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let format = match extension.as_str() {
        _ if packet => packets::recognizes(&bytes, path).then_some(Format::PacketCapture),
        _ if can => can::identify(&bytes).map(|_| Format::CanCapture),
        "ts" => touchstone::recognizes(&bytes, path).then_some(Format::Touchstone),
        "dsl" => (bytes.starts_with(b"PK\x03\x04") && logic::probe(&file, path))
            .then_some(Format::LogicCapture),
        "bin" if logic::matches_binary_header(&bytes) => Some(Format::LogicCapture),
        _ => scope::matches_header_with_len(&bytes, length).then_some(Format::Oscilloscope),
    }?;
    source.check_file(&file).ok()?;
    Some(format)
}

/// Returns whether a path uses a Gaussian-capable scene container extension.
///
/// GLB may instead contain meshes; loading classifies it on the worker thread.
pub fn is_splat_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            ["ply", "splat", "sog", "spz", "glb"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// Returns whether a path has the conventional MIDI filename suffix.
pub fn is_midi(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn touchstone_detection_leaves_typescript_sources_routed_as_text() {
        let directory = tempfile::tempdir().unwrap();
        let sweep = directory.path().join("sweep.ts");
        std::fs::write(
            &sweep,
            "[Version] 2.0\n# Hz S RI R 50\n[Number of Ports] 1\n[Number of Frequencies] 1\n[Network Data]\n1000000 0.5 0\n[End]\n",
        )
        .unwrap();
        let script = directory.path().join("script.ts");
        std::fs::write(&script, "export const sweeps = [1, 2, 3];\n").unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            recognize_engineering(&sweep, &cancel),
            Some(Format::Touchstone)
        );
        assert_eq!(recognize_engineering(&script, &cancel), None);
        assert!(crate::text_format::is_typescript_source(&script));
    }

    #[test]
    fn recognized_scope_csv_changes_browser_routing_without_claiming_other_csv_or_binary() {
        let directory = tempfile::tempdir().unwrap();
        let waveform = directory.path().join("waveform.csv");
        std::fs::write(
            &waveform,
            "X,CH1,Start,Increment,\nSequence,Volt,-1e-3,1e-6\n0,1.5\n1,0.5\n",
        )
        .unwrap();
        let spreadsheet = directory.path().join("measurements.csv");
        std::fs::write(&spreadsheet, "X,Start,Increment\n1,2,3\n").unwrap();
        let binary = directory.path().join("ordinary.bin");
        std::fs::write(&binary, [0; 1024]).unwrap();

        let cancel = AtomicBool::new(false);
        let inventory = crate::file_inventory::read_directory(directory.path(), &cancel).unwrap();
        for (path, expected) in [
            (waveform, Some(Format::Oscilloscope)),
            (spreadsheet, Some(Format::Spreadsheet)),
            (binary, None),
        ] {
            let entry = inventory
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap();
            assert_eq!(entry.document_format(), expected);
        }
    }

    #[test]
    fn classification_and_preview_source_share_the_same_format_inventory() {
        for (name, format) in [
            ("font.OTC", Format::Font),
            ("a.IPYNB", Format::Notebook),
            ("a.EPUB", Format::Book),
            ("a.CBZ", Format::Book),
            ("a.EML", Format::Mail),
            ("a.ICS", Format::Calendar),
            ("a.DB3", Format::Database),
            ("a.GPX", Format::Geography),
            ("a.GEOJSON", Format::Geography),
            ("a.RCTRK", Format::Geography),
            ("a.GCODE", Format::Toolpath),
            ("a.PES", Format::Embroidery),
            ("a.VIP", Format::Embroidery),
            ("a.SF2", Format::Multisample),
            ("a.SFZ", Format::Multisample),
            ("a.NKI", Format::Multisample),
            ("a.DSPRESET", Format::Multisample),
            ("a.DSLIBRARY", Format::Multisample),
            ("a.MULTISAMPLE", Format::Multisample),
            ("a.VTT", Format::Subtitle),
            ("a.VCV", Format::VcvPatch),
            ("a.VCVSS", Format::VcvPatch),
            ("budget.CSV", Format::Spreadsheet),
            ("budget.TSV", Format::Spreadsheet),
            ("budget.XLS", Format::Spreadsheet),
            ("budget.XLSX", Format::Spreadsheet),
            ("budget.XLSB", Format::Spreadsheet),
            ("budget.ODS", Format::Spreadsheet),
        ] {
            assert_eq!(Format::for_path(Path::new(name)), Some(format));
        }
        assert_eq!(Format::for_path(Path::new("ordinary.txt")), None);
        assert_eq!(Format::for_path(Path::new("ordinary.wav")), None);
        for path in [
            "ordinary.xml",
            "ordinary.exs",
            "ordinary.xpm",
            "ordinary.preset",
            "ordinary.iso",
        ] {
            assert_eq!(Format::for_path(Path::new(path)), None);
        }
    }
}
