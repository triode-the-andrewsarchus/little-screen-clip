//! Cheap format hints shared by strips, row admission and studio dispatch.
//!
//! These hints do not establish content identity, readability or edit support.
//! Workers validate ordinary sources and decode them before publishing prepared
//! documents. Ambiguous sampler names require positive content identification;
//! they retain their ordinary routing until a worker supplies that evidence.
use std::{
    borrow::Cow,
    fs::File,
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
    AnalysisSession,
    PowerRecording,
    Touchstone,
    LogicCapture,
    PacketCapture,
    CanCapture,
    PulseSignals,
    TagCapture,
    OneWire,
    RadioProgramming,
    Flowgraph,
    IqCapture,
}
impl Format {
    /// Returns the shared filename hint without reading or admitting the source.
    pub fn for_path(path: &Path) -> Option<Self> {
        match lowercase_extension(path)?.as_ref() {
            "ttf" | "ttc" | "otf" | "otc" | "woff" | "woff2" => Some(Self::Font),
            "ipynb" => Some(Self::Notebook),
            "epub" | "cbz" => Some(Self::Book),
            "eml" => Some(Self::Mail),
            "ics" => Some(Self::Calendar),
            "sqlite" | "sqlite3" | "db" | "db3" => Some(Self::Database),
            "gpx" | "geojson" | "rctrk" => Some(Self::Geography),
            "gcode" => Some(Self::Toolpath),
            "wfm" => Some(Self::Oscilloscope),
            "clip-analysis" => Some(Self::AnalysisSession),
            "jls" => Some(Self::PowerRecording),
            "sr" | "sal" | "vcd" => Some(Self::LogicCapture),
            "pcap" | "pcapng" => Some(Self::PacketCapture),
            "ir" => Some(Self::PulseSignals),
            "chirp" => Some(Self::RadioProgramming),
            "grc" => Some(Self::Flowgraph),
            "sigmf-meta" | "sigmf-data" => Some(Self::IqCapture),
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
    /// Types whose previews show instrument or engineering data.
    pub fn is_engineering(self) -> bool {
        matches!(
            self,
            Self::Oscilloscope
                | Self::AnalysisSession
                | Self::PowerRecording
                | Self::Touchstone
                | Self::LogicCapture
                | Self::PacketCapture
                | Self::CanCapture
                | Self::PulseSignals
                | Self::TagCapture
                | Self::OneWire
                | Self::RadioProgramming
                | Self::Flowgraph
                | Self::IqCapture
        )
    }
}

fn lowercase_extension(path: &Path) -> Option<Cow<'_, str>> {
    let extension = path.extension()?.to_str()?;
    Some(if extension.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(extension.to_ascii_lowercase())
    } else {
        Cow::Borrowed(extension)
    })
}

/// Whether an extension requires positive content evidence to override routing.
pub fn ambiguous_engineering_path(path: &Path) -> bool {
    EngineeringProbe::for_path(path).is_some()
}

/// Check a bounded, identity-verified prefix on a content worker.
///
/// No hint leaves existing routing in place. The successful decoder must still
/// validate its own pinned input, including any format-specific offsets.
pub fn recognize_engineering(path: &Path, cancel: &AtomicBool) -> Option<Format> {
    let probe = EngineeringProbe::for_path(path)?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let (file, source) = crate::file_source::ReadCheck::default()
        .open(path, None, cancel)
        .ok()?;
    let length = file.metadata().ok()?.len();
    let mut bytes = Vec::new();
    (&file)
        .take(probe.byte_limit() as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let format = probe.identify(path, &file, &bytes, length)?;
    source.check_file(&file).ok()?;
    Some(format)
}

enum EngineeringProbe {
    Csv,
    Binary,
    Touchstone,
    DsView,
    FlipperRadio,
    Icom,
    NetworkCapture,
    Can,
    Trc,
    Trace,
    Tag(Format),
}

impl EngineeringProbe {
    fn for_path(path: &Path) -> Option<Self> {
        use crate::engineering::{packets, tags};

        Some(match lowercase_extension(path)?.as_ref() {
            "csv" => Self::Csv,
            "bin" => Self::Binary,
            "ts" => Self::Touchstone,
            "dsl" => Self::DsView,
            "sub" => Self::FlipperRadio,
            "icf" => Self::Icom,
            "pcap" | "pcapng" => return None,
            "asc" | "blf" | "log" | "candump" => Self::Can,
            "trc" => Self::Trc,
            "trace" => Self::Trace,
            "ibtn" => Self::Tag(Format::OneWire),
            _ if tags::is_candidate_path(path) => Self::Tag(Format::TagCapture),
            _ if packets::is_candidate_path(path) => Self::NetworkCapture,
            _ => return None,
        })
    }

    fn byte_limit(&self) -> usize {
        use crate::engineering::{can, pulses, scope};

        match self {
            Self::Csv | Self::Binary => scope::HEADER_BYTES,
            Self::Touchstone | Self::NetworkCapture => 64 * 1024,
            Self::DsView => 4,
            Self::FlipperRadio => pulses::HEADER_BYTES,
            Self::Icom | Self::Can => can::HEADER_BYTES,
            Self::Trc => can::HEADER_BYTES.max(scope::HEADER_BYTES),
            Self::Trace | Self::Tag(_) => 32 * 1024,
        }
    }

    fn identify(&self, path: &Path, file: &File, bytes: &[u8], length: u64) -> Option<Format> {
        use crate::engineering::{can, logic, packets, pulses, radio, scope, tags, touchstone};

        let scope_format =
            || scope::matches_header_with_len(bytes, length).then_some(Format::Oscilloscope);
        let can_format = || can::identify(bytes).map(|_| Format::CanCapture);
        let tag_format =
            |format| tags::recognizes_with_length(bytes, path, length).then_some(format);
        match self {
            Self::Csv => radio::is_chirp_csv(bytes)
                .then_some(Format::RadioProgramming)
                .or_else(scope_format),
            Self::Binary => logic::matches_binary_header(bytes)
                .then_some(Format::LogicCapture)
                .or_else(scope_format),
            Self::Touchstone => touchstone::recognizes(bytes, path).then_some(Format::Touchstone),
            Self::DsView => (bytes.starts_with(b"PK\x03\x04") && logic::probe(file, path))
                .then_some(Format::LogicCapture),
            Self::FlipperRadio => matches!(
                pulses::identify(bytes),
                Some(pulses::Format::RadioRaw | pulses::Format::RadioDecoded)
            )
            .then_some(Format::PulseSignals),
            Self::Icom => radio::is_icf(bytes).then_some(Format::RadioProgramming),
            Self::NetworkCapture => {
                packets::recognizes(bytes, path).then_some(Format::PacketCapture)
            }
            Self::Can => can_format(),
            Self::Trc => scope_format().or_else(can_format),
            Self::Trace => can_format().or_else(|| tag_format(Format::TagCapture)),
            Self::Tag(format) => tag_format(*format),
        }
    }
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
    fn shared_suffixes_route_to_engineering_only_after_content_checks() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let ordinary = directory.path().join("letter.eml");
        std::fs::write(
            &ordinary,
            "From: a@example.invalid\nSubject: Test\n\nOrdinary email\n",
        )
        .unwrap();
        assert_eq!(recognize_engineering(&ordinary, &cancel), None);
        assert_eq!(Format::for_path(&ordinary), Some(Format::Mail));

        let memory = directory.path().join("blocks.eml");
        std::fs::write(&memory, format!("{}\n", "00".repeat(16)).repeat(64)).unwrap();
        assert_eq!(
            recognize_engineering(&memory, &cancel),
            Some(Format::TagCapture)
        );

        let rf_trace = directory.path().join("rf.trace");
        let mut bytes = Vec::new();
        for time in [100_u32, 120] {
            bytes.extend(time.to_le_bytes());
            bytes.extend(5_u16.to_le_bytes());
            bytes.extend(1_u16.to_le_bytes());
            bytes.extend([0, 0]);
        }
        std::fs::write(&rf_trace, bytes).unwrap();
        assert_eq!(
            recognize_engineering(&rf_trace, &cancel),
            Some(Format::TagCapture)
        );

        let subtitles = directory.path().join("movie.sub");
        std::fs::write(&subtitles, "{20}{40}Subtitles\n").unwrap();
        assert_eq!(recognize_engineering(&subtitles, &cancel), None);
        let signals = directory.path().join("timing.sub");
        std::fs::write(
            &signals,
            "Filetype: Flipper SubGhz RAW File\nVersion: 1\nProtocol: RAW\nRAW_Data: 40 -20\n",
        )
        .unwrap();
        assert_eq!(
            recognize_engineering(&signals, &cancel),
            Some(Format::PulseSignals)
        );
    }

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
