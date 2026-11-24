//! SPICE circuit netlists (`.cir`, `.sp`, `.spice`), SPICE/LTspice simulation
//! output waveforms (`.raw`), and LTspice plot settings (`.plt`).

mod peaks;
mod sweeps;

pub use peaks::{SpicePreviewSample, SpiceRawTrace};

use peaks::{WaveformPreview, displayed_value};

use super::{StripSummary, bounded_source_excerpt, check_cancelled, pixmap_to_thumbnail};
use crate::ImageThumbnail;
use dune_netlist::IncludeLoader;
use dune_netlist::ir::Analysis;
use dune_raw::Rawfile;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

const SCOPE_BG: [u8; 4] = [16, 20, 26, 255];
const MAX_NETLIST_LIST: usize = 2048;
const MAX_NETLIST_NODES: usize = 4096;
const MAX_DECODED_TRACES: usize = 64;
const MAX_WAVEFORM_POINTS: usize = 4096;
const MAX_RAW_PLOTS: usize = 16;

pub const TRACE_PALETTE: [[u8; 3]; 8] = [
    [98, 224, 134],
    [102, 186, 255],
    [255, 198, 92],
    [244, 114, 132],
    [194, 148, 255],
    [94, 228, 224],
    [255, 146, 86],
    [224, 232, 240],
];

struct NoIncludeLoader;

impl IncludeLoader for NoIncludeLoader {
    fn load(&mut self, path: &Path, _limit: u64) -> Result<String, String> {
        Err(format!(
            "External include '{}' is listed as metadata only",
            path.display()
        ))
    }
}

#[derive(Clone, Debug)]
pub struct SpiceSubcktInfo {
    pub name: String,
    pub ports: Vec<String>,
    pub element_count: usize,
}

#[derive(Clone, Debug)]
pub struct SpiceModelInfo {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct SpiceComponentInfo {
    pub name: String,
    pub kind: &'static str,
    pub nodes: Vec<String>,
}

/// Decoded SPICE netlist document (`.cir`, `.sp`, `.spice`).
#[derive(Clone, Debug)]
pub struct SpiceNetlistDocument {
    pub title: String,
    pub node_count: usize,
    pub nodes: Vec<String>,
    pub component_count: usize,
    pub components: Vec<SpiceComponentInfo>,
    pub category_counts: Vec<(&'static str, usize)>,
    pub subcircuits: Vec<SpiceSubcktInfo>,
    pub models: Vec<SpiceModelInfo>,
    pub analyses: Vec<String>,
    pub includes: Vec<String>,
    pub diagnostics: Vec<String>,
    pub source_excerpt: String,
}

impl SpiceNetlistDocument {
    pub fn parse(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "SPICE netlist parse cancelled")?;
        let text = decode_spice_text(bytes);
        if text.trim().is_empty() {
            return Err("SPICE netlist is empty".into());
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("circuit.cir");
        let outcome =
            dune_netlist::parse_with_loader(Path::new(name), text.clone(), &mut NoIncludeLoader);
        check_cancelled(cancelled, "SPICE netlist parse cancelled")?;

        let use_card_elements = outcome.netlist.elements.is_empty();
        let scanned = scan_spice_cards(&text, use_card_elements, cancelled)?;
        let title = pick_netlist_title(&outcome.netlist.title, &scanned.title, path);

        let mut elements = scanned.elements;
        for (index, element) in outcome.netlist.elements.iter().enumerate() {
            if index.is_multiple_of(512) {
                check_cancelled(cancelled, "SPICE netlist summary cancelled")?;
            }
            elements.record(&element.name, element.nodes.iter().map(String::as_str));
        }
        let models = merge_models(&outcome.netlist.models, scanned.models);
        let analyses = merge_analyses(&outcome.netlist.analyses, scanned.analyses);

        let diagnostics: Vec<String> = outcome
            .diags
            .iter()
            .take(16)
            .map(|d| d.render(&outcome.sources))
            .collect();

        Ok(Self {
            title,
            node_count: elements.nodes.len(),
            nodes: elements.nodes.into_iter().take(MAX_NETLIST_NODES).collect(),
            component_count: elements.categories.values().sum(),
            components: elements.components,
            category_counts: elements.categories.into_iter().collect(),
            subcircuits: scanned.subcircuits,
            models,
            analyses,
            includes: scanned.includes,
            diagnostics,
            source_excerpt: bounded_source_excerpt(&text),
        })
    }

    pub fn summary_line(&self) -> String {
        format!(
            "SPICE {} · {} components · {} nodes · {} subckts · {} analyses",
            self.title,
            self.component_count,
            self.node_count,
            self.subcircuits.len(),
            self.analyses.len()
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let an = if self.analyses.is_empty() {
            "No analysis".to_string()
        } else {
            self.analyses.join(", ")
        };
        StripSummary {
            format: super::EdaFormat::SpiceNetlist,
            headline: format!("SPICE Netlist · {}", self.title),
            specs: format!(
                "{} components · {} nodes · {} subcircuits · {an}",
                self.component_count,
                self.node_count,
                self.subcircuits.len()
            ),
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "SPICE netlist render cancelled")?;
        let w = max_size[0].clamp(64, 512);
        let h = max_size[1].clamp(48, 512);
        let mut pixmap = Pixmap::new(w, h)
            .ok_or_else(|| "Could not allocate SPICE netlist preview".to_string())?;
        pixmap.fill(Color::from_rgba8(
            SCOPE_BG[0],
            SCOPE_BG[1],
            SCOPE_BG[2],
            SCOPE_BG[3],
        ));

        draw_component_distribution(&mut pixmap, self);
        Ok(pixmap_to_thumbnail(pixmap))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawAnalysisKind {
    Transient,
    AcSweep,
    DcSweep,
    DcOperatingPoint,
    Noise,
    Other,
}

impl RawAnalysisKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Transient => "Transient",
            Self::AcSweep => "AC Sweep",
            Self::DcSweep => "DC Sweep",
            Self::DcOperatingPoint => "DC Operating Point",
            Self::Noise => "Noise",
            Self::Other => "Simulation",
        }
    }

    fn classify(plot_name: &str, is_complex: bool, npoints: usize) -> Self {
        let lower = plot_name.to_ascii_lowercase();
        if lower.contains("operating point") {
            Self::DcOperatingPoint
        } else if lower.contains("transient") {
            Self::Transient
        } else if lower.contains("ac analysis") || is_complex {
            Self::AcSweep
        } else if lower.contains("dc transfer") || lower.contains("dc sweep") {
            Self::DcSweep
        } else if lower.contains("noise") {
            Self::Noise
        } else if npoints == 1 {
            Self::DcOperatingPoint
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpiceRawPlot {
    pub title: String,
    pub date: String,
    pub plot_name: String,
    pub kind: RawAnalysisKind,
    pub is_complex: bool,
    pub total_points: usize,
    pub total_vars: usize,
    pub x_name: String,
    pub x_unit: String,
    /// Minimum and maximum finite coordinates on the source axis.
    pub x_range: Option<[f64; 2]>,
    pub traces: Vec<SpiceRawTrace>,
}

impl SpiceRawPlot {
    /// Reports whether the preview uses a logarithmic frequency axis.
    pub fn log_x(&self) -> bool {
        self.kind == RawAnalysisKind::AcSweep && self.x_range.is_some_and(|[lo, _]| lo > 0.0)
    }
}

/// Decoded SPICE / LTspice `.raw` simulation waveform document.
#[derive(Clone, Debug)]
pub struct SpiceRawDocument {
    pub title: String,
    pub plots: Vec<SpiceRawPlot>,
    pub warnings: Vec<String>,
}

impl SpiceRawDocument {
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "SPICE raw parse cancelled")?;
        let Some(fast_access) = inspect_raw_prefix(bytes) else {
            return Err("File is not a SPICE/LTspice .raw simulation output".into());
        };
        if fast_access {
            return parse_ltspice_raw(bytes, cancelled);
        }
        if let Ok(raw) = Rawfile::parse(bytes)
            && !raw.plots.is_empty()
            && !raw.plots.iter().all(|p| p.data.num_points() == 0)
        {
            // dune-raw accepts unknown flags and assumes interleaved records.
            if raw
                .warnings
                .iter()
                .any(|warning| warning.eq_ignore_ascii_case("unknown flag fastaccess"))
            {
                // A FastAccess flag outside the qualified first header cannot
                // be associated with the right plot by the interleaved reader.
                return Err("LTspice FastAccess requires a single identified binary plot".into());
            }
            return convert_dune_rawfile(raw, cancelled);
        }
        parse_ltspice_raw(bytes, cancelled)
    }

    pub fn primary(&self) -> Option<&SpiceRawPlot> {
        self.plots.first()
    }

    pub fn summary_line(&self) -> String {
        let Some(p) = self.primary() else {
            return format!("SPICE Waveforms · {}", self.title);
        };
        format!(
            "SPICE {} · {} · {} vars · {} pts",
            p.title,
            p.kind.label(),
            p.total_vars,
            p.total_points
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let Some(p) = self.primary() else {
            return StripSummary {
                format: super::EdaFormat::SpiceRaw,
                headline: format!("SPICE Waveforms · {}", self.title),
                specs: "Empty simulation output".into(),
                detail: self.summary_line(),
            };
        };
        let x_span = p.x_range.map_or_else(
            || p.x_name.clone(),
            |[a, b]| format!("{} [{a:.3e}..{b:.3e}]", p.x_name),
        );
        StripSummary {
            format: super::EdaFormat::SpiceRaw,
            headline: format!("{} · {}", p.kind.label(), p.title),
            specs: format!(
                "{} traces · {} pts · {x_span}",
                p.traces.len(),
                p.total_points
            ),
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "SPICE raw render cancelled")?;
        let w = max_size[0].clamp(64, 1024);
        let h = max_size[1].clamp(48, 1024);
        let mut pixmap =
            Pixmap::new(w, h).ok_or_else(|| "Could not allocate waveform canvas".to_string())?;
        pixmap.fill(Color::from_rgba8(
            SCOPE_BG[0],
            SCOPE_BG[1],
            SCOPE_BG[2],
            SCOPE_BG[3],
        ));
        draw_scope_grid(&mut pixmap);

        if let Some(plot) = self.primary() {
            if plot.kind == RawAnalysisKind::DcOperatingPoint {
                draw_operating_point_bars(&mut pixmap, plot);
            } else {
                draw_waveform_curves(&mut pixmap, plot, cancelled)?;
            }
        }
        Ok(pixmap_to_thumbnail(pixmap))
    }
}

#[derive(Clone, Debug, Default)]
pub struct LtspicePane {
    pub log_x: bool,
    pub log_y: bool,
    pub traces: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LtspiceSection {
    pub analysis: String,
    pub panes: Vec<LtspicePane>,
}

/// Decoded LTspice `.plt` plot settings document.
#[derive(Clone, Debug)]
pub struct LtspicePlotDocument {
    pub sections: Vec<LtspiceSection>,
    pub trace_count: usize,
    pub source_excerpt: String,
}

impl LtspicePlotDocument {
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "LTspice .plt parse cancelled")?;
        let text = decode_spice_text(bytes);
        let mut sections = Vec::new();
        let mut brace_depth = 0_usize;

        for (index, raw) in text.lines().enumerate() {
            if index >= 4096 {
                return Err("LTspice plot settings exceed the preview line limit".into());
            }
            if index.is_multiple_of(512) {
                check_cancelled(cancelled, "LTspice .plt parse cancelled")?;
            }
            let line = raw.trim();
            if let Some(analysis) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                sections.push(LtspiceSection {
                    analysis: analysis.to_owned(),
                    panes: Vec::new(),
                });
                brace_depth = 0;
                continue;
            }
            let Some(section) = sections.last_mut() else {
                continue;
            };
            if line.starts_with('{') && brace_depth == 1 {
                section
                    .panes
                    .push(extract_ltspice_pane(line).unwrap_or_default());
            } else if brace_depth >= 2
                && let Some(pane) = section.panes.last_mut()
            {
                apply_pane_line(pane, line);
            }
            brace_depth = advance_brace_depth(brace_depth, line);
        }

        if sections.is_empty() {
            return Err("No LTspice plot analysis sections found".into());
        }
        let trace_count = sections
            .iter()
            .flat_map(|section| &section.panes)
            .map(|pane| pane.traces.len())
            .sum();

        Ok(Self {
            sections,
            trace_count,
            source_excerpt: bounded_source_excerpt(&text),
        })
    }

    pub fn pane_count(&self) -> usize {
        self.sections
            .iter()
            .map(|section| section.panes.len())
            .sum()
    }

    fn analysis_label(&self) -> String {
        match self.sections.as_slice() {
            [section] => section.analysis.clone(),
            sections => format!("{} analyses", sections.len()),
        }
    }

    pub fn summary_line(&self) -> String {
        format!(
            "LTspice Plot · {} · {} panes · {} traces",
            self.analysis_label(),
            self.pane_count(),
            self.trace_count
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let preview_traces: Vec<&str> = self
            .sections
            .iter()
            .flat_map(|section| &section.panes)
            .flat_map(|p| p.traces.iter().map(String::as_str))
            .take(4)
            .collect();
        let list = if preview_traces.is_empty() {
            "No traces configured".to_string()
        } else {
            preview_traces.join(", ")
        };
        StripSummary {
            format: super::EdaFormat::LtspicePlot,
            headline: format!("LTspice Plot · {}", self.analysis_label()),
            specs: format!(
                "{} panes · {} traces · {list}",
                self.pane_count(),
                self.trace_count
            ),
            detail: self.summary_line(),
        }
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "LTspice .plt render cancelled")?;
        let w = max_size[0].clamp(64, 1024);
        let h = max_size[1].clamp(48, 1024);
        let mut pixmap = Pixmap::new(w, h)
            .ok_or_else(|| "Could not allocate LTspice plot preview".to_string())?;
        pixmap.fill(Color::from_rgba8(
            SCOPE_BG[0],
            SCOPE_BG[1],
            SCOPE_BG[2],
            SCOPE_BG[3],
        ));
        draw_ltspice_plt_panes(&mut pixmap, self);
        Ok(pixmap_to_thumbnail(pixmap))
    }
}

/// Returns whether a `.raw` buffer is a SPICE/LTspice waveform file rather than camera RAW.
pub fn is_spice_raw_bytes(bytes: &[u8]) -> bool {
    inspect_raw_prefix(bytes).is_some()
}

// Once the prefix qualifies as a waveform, report whether its first header
// specifies FastAccess. Stop at the data marker even when looking for flags.
fn inspect_raw_prefix(bytes: &[u8]) -> Option<bool> {
    if bytes.len() < 16 || looks_like_camera_raw_magic(bytes) {
        return None;
    }
    let prefix = &bytes[..bytes.len().min(4096)];
    let header = decode_spice_text(prefix);
    let mut title = false;
    let mut plot_name = false;
    let mut variable_count = false;
    let mut fast_access = false;
    for (key, value) in header
        .lines()
        .filter_map(|line| line.trim().split_once(':'))
    {
        match key.trim().to_ascii_lowercase().as_str() {
            "title" => title = true,
            "plotname" => plot_name = !value.trim().is_empty(),
            "no. variables" => variable_count = value.trim().parse::<usize>().is_ok_and(|n| n > 0),
            "flags" => {
                fast_access = value
                    .split_ascii_whitespace()
                    .any(|flag| flag.eq_ignore_ascii_case("fastaccess"))
            }
            "values" | "binary" => break,
            _ => {}
        }
    }
    (title && plot_name && variable_count).then_some(fast_access)
}

/// Returns whether an on-disk `.raw` path holds a SPICE/LTspice simulation output.
pub fn is_spice_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("raw"))
        && crate::file_source::read_file_bytes(path, 8192, false, true)
            .is_ok_and(|bytes| is_spice_raw_bytes(&bytes))
}

/// Returns whether a `.plt` buffer uses LTspice plot settings syntax instead of HPGL.
pub fn looks_like_ltspice_plt(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(8192)];
    let text = decode_spice_text(prefix);
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(section) = lines.next().and_then(|line| line.strip_prefix('[')) else {
        return false;
    };
    let Some(analysis) = section.strip_suffix(']') else {
        return false;
    };
    matches!(
        analysis.to_ascii_lowercase().as_str(),
        "transient analysis"
            | "ac analysis"
            | "dc sweep"
            | "dc transfer characteristic"
            | "noise analysis"
            | "dc operating point"
            | "fft"
    ) || lines.any(|line| strip_key_eq_or_colon(line, "npanes").is_some())
}

fn looks_like_camera_raw_magic(bytes: &[u8]) -> bool {
    let starts = [
        b"II*\0".as_slice(),
        b"II+\0".as_slice(),
        b"MM\0*".as_slice(),
        b"MM\0+".as_slice(),
        b"FUJIFILM".as_slice(),
        b"IIRO".as_slice(),
        b"IIRS".as_slice(),
        b"IIU\0".as_slice(),
        b"FOVb".as_slice(),
    ];
    if starts.iter().any(|m| bytes.starts_with(m)) {
        return true;
    }
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

fn decode_spice_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16_le(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16_be(rest);
    }
    if is_likely_utf16_le(bytes) {
        return decode_utf16_le(bytes);
    }
    String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)).into_owned()
}

fn is_likely_utf16_le(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(64)];
    if sample.len() < 8 {
        return false;
    }
    let odd_zeros = sample
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|[lo, hi]| *lo != 0 && *hi == 0)
        .count();
    odd_zeros * 4 >= sample.len()
}

fn decode_utf16_le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_le_bytes(*chunk))
        .collect();
    String::from_utf16_lossy(&units)
}

fn decode_utf16_be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| u16::from_be_bytes(*chunk))
        .collect();
    String::from_utf16_lossy(&units)
}

struct ScannedNetlist {
    title: String,
    elements: ElementSummary,
    subcircuits: Vec<SpiceSubcktInfo>,
    models: Vec<SpiceModelInfo>,
    analyses: Vec<String>,
    includes: Vec<String>,
}

#[derive(Default)]
struct ElementSummary {
    nodes: BTreeSet<String>,
    components: Vec<SpiceComponentInfo>,
    categories: BTreeMap<&'static str, usize>,
}

impl ElementSummary {
    fn record<'a>(&mut self, name: &str, nodes: impl Iterator<Item = &'a str>) {
        let Some(prefix) = name.chars().next() else {
            return;
        };
        let (kind, category, _) = spice_device_kind(prefix);
        *self.categories.entry(category).or_default() += 1;
        let save = self.components.len() < MAX_NETLIST_LIST;
        let mut connections = Vec::new();
        for node in nodes {
            self.nodes.insert(node.to_ascii_lowercase());
            if save {
                connections.push(node.to_owned());
            }
        }
        if save {
            self.components.push(SpiceComponentInfo {
                name: name.to_owned(),
                kind,
                nodes: connections,
            });
        }
    }
}

fn join_logical_lines(text: &str) -> Vec<String> {
    let mut cards: Vec<String> = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if let Some(cont) = trimmed.strip_prefix('+') {
            if let Some(prev) = cards.last_mut() {
                prev.push(' ');
                prev.push_str(cont.trim_start());
            }
        } else {
            cards.push(trimmed.to_string());
        }
    }
    cards
}

fn spice_device_kind(prefix: char) -> (&'static str, &'static str, usize) {
    match prefix.to_ascii_uppercase() {
        'R' => ("Resistor", "Resistors", 2),
        'C' => ("Capacitor", "Capacitors", 2),
        'L' => ("Inductor", "Inductors", 2),
        'D' => ("Diode", "Semiconductors", 2),
        'Q' => ("BJT", "Semiconductors", 3),
        'M' => ("MOSFET", "Semiconductors", 4),
        'J' => ("JFET", "Semiconductors", 3),
        'Z' => ("MESFET", "Semiconductors", 3),
        'V' => ("Voltage Source", "Sources", 2),
        'I' => ("Current Source", "Sources", 2),
        'E' | 'F' | 'G' | 'H' | 'B' => ("Controlled/Behav", "Controlled/Behav", 2),
        'X' => ("Subcircuit Instance", "Subckt Instances", 1),
        _ => ("Device", "Other Devices", 2),
    }
}

fn is_decorative_comment(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_alphanumeric)
}

fn update_title_candidate(idx: usize, trimmed: &str, title: &mut String) {
    if !title.is_empty() {
        return;
    }
    let is_comment_header = idx < 4 && trimmed.starts_with('*');
    let is_first_title_card = idx == 0 && !trimmed.is_empty() && !trimmed.starts_with('.');
    if !is_comment_header && !is_first_title_card {
        return;
    }
    let candidate = trimmed.trim_start_matches('*').trim();
    if !is_decorative_comment(candidate) {
        *title = candidate.to_string();
    }
}

fn try_consume_control_or_title(
    trimmed: &str,
    in_control: &mut bool,
    dot_title: &mut String,
) -> bool {
    let lower = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match lower.as_str() {
        ".control" => {
            *in_control = true;
            true
        }
        ".endc" => {
            *in_control = false;
            true
        }
        ".title" => {
            if dot_title.is_empty() {
                *dot_title = trimmed
                    .get(".title".len()..)
                    .unwrap_or("")
                    .trim()
                    .to_string();
            }
            true
        }
        _ => false,
    }
}

fn scan_spice_cards(
    text: &str,
    use_card_elements: bool,
    cancelled: &AtomicBool,
) -> Result<ScannedNetlist, String> {
    let cards = join_logical_lines(text);
    let mut title = String::new();
    let mut dot_title = String::new();
    let mut elements = ElementSummary::default();
    let mut subcircuits = Vec::new();
    let mut active_subckt: Option<SpiceSubcktInfo> = None;
    let mut in_control = false;
    let mut models = Vec::new();
    let mut analyses = Vec::new();
    let mut includes = Vec::new();

    for (idx, line) in cards.iter().enumerate() {
        if idx.is_multiple_of(512) {
            check_cancelled(cancelled, "SPICE scan cancelled")?;
        }
        let trimmed = line.trim();
        update_title_candidate(idx, trimmed, &mut title);
        if trimmed.is_empty() || trimmed.starts_with('*') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('.') {
            if !try_consume_control_or_title(trimmed, &mut in_control, &mut dot_title)
                && !in_control
            {
                scan_dot_directive(
                    trimmed,
                    &mut active_subckt,
                    &mut subcircuits,
                    &mut models,
                    &mut analyses,
                    &mut includes,
                );
            }
            continue;
        }
        if in_control {
            continue;
        }
        if let Some(sub) = &mut active_subckt {
            sub.element_count += 1;
            continue;
        }
        if use_card_elements && idx != 0 {
            record_top_element(trimmed, &mut elements);
        }
    }
    if let Some(sub) = active_subckt {
        subcircuits.push(sub);
    }
    if !dot_title.is_empty() {
        title = dot_title;
    }

    Ok(ScannedNetlist {
        title,
        elements,
        subcircuits,
        models,
        analyses,
        includes,
    })
}

fn scan_dot_directive(
    line: &str,
    active_subckt: &mut Option<SpiceSubcktInfo>,
    subcircuits: &mut Vec<SpiceSubcktInfo>,
    models: &mut Vec<SpiceModelInfo>,
    analyses: &mut Vec<String>,
    includes: &mut Vec<String>,
) {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let Some(&cmd) = tokens.first() else {
        return;
    };
    let lower = cmd.to_ascii_lowercase();
    match lower.as_str() {
        ".subckt" if tokens.len() >= 2 => {
            *active_subckt = Some(SpiceSubcktInfo {
                name: tokens[1].to_string(),
                ports: tokens[2..].iter().map(|s| (*s).to_string()).collect(),
                element_count: 0,
            });
        }
        ".ends" => {
            if let Some(sub) = active_subckt.take() {
                subcircuits.push(sub);
            }
        }
        ".model" if tokens.len() >= 3 && models.len() < MAX_NETLIST_LIST => {
            models.push(SpiceModelInfo {
                name: tokens[1].to_string(),
                kind: tokens[2].split('(').next().unwrap_or(tokens[2]).to_string(),
            });
        }
        ".include" | ".inc" | ".lib" if tokens.len() >= 2 => {
            includes.push(tokens[1..].join(" ").trim_matches('"').to_string());
        }
        ".tran" | ".ac" | ".dc" | ".op" | ".noise" | ".tf" | ".sens" | ".step" | ".meas" => {
            analyses.push(line.to_string());
        }
        _ => {}
    }
}

fn record_top_element(line: &str, elements: &mut ElementSummary) {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let Some(name) = tokens.first().copied() else {
        return;
    };
    let Some(first_ch) = name.chars().next() else {
        return;
    };
    let (_, _, min_nodes) = spice_device_kind(first_ch);

    let end = if first_ch.eq_ignore_ascii_case(&'X') {
        tokens.len().saturating_sub(1).max(2)
    } else {
        (1 + min_nodes).min(tokens.len())
    };
    let nodes = tokens
        .get(1..end)
        .unwrap_or(&[])
        .iter()
        .copied()
        .filter(|n| !n.contains('='));
    elements.record(name, nodes);
}

fn pick_netlist_title(ir_title: &str, scan_title: &str, path: &Path) -> String {
    if !scan_title.is_empty() {
        return scan_title.to_string();
    }
    let ir = ir_title.trim();
    let ir = if ir
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case(".title"))
    {
        ir.get(6..).unwrap_or("").trim()
    } else {
        ir
    };
    let cleaned = ir.trim_start_matches('*').trim();
    if !cleaned.is_empty() && ir != "*untitled*" && !is_decorative_comment(cleaned) {
        return cleaned.to_string();
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("SPICE Netlist")
        .to_string()
}

fn merge_models(
    ir_models: &[dune_netlist::ir::Model],
    mut scanned: Vec<SpiceModelInfo>,
) -> Vec<SpiceModelInfo> {
    let mut seen: BTreeSet<String> = scanned
        .iter()
        .map(|m| m.name.to_ascii_lowercase())
        .collect();
    for m in ir_models.iter().take(MAX_NETLIST_LIST) {
        if seen.insert(m.name.to_ascii_lowercase()) && scanned.len() < MAX_NETLIST_LIST {
            scanned.push(SpiceModelInfo {
                name: m.name.clone(),
                kind: m.mtype.clone(),
            });
        }
    }
    scanned
}

fn merge_analyses(ir_analyses: &[Analysis], scanned: Vec<String>) -> Vec<String> {
    if !scanned.is_empty() {
        return scanned;
    }
    ir_analyses.iter().map(format_ir_analysis).collect()
}

fn format_ir_analysis(analysis: &Analysis) -> String {
    match analysis {
        Analysis::Op { .. } => ".op".into(),
        Analysis::Dc { sweeps, .. } => format!(".dc ({} sweep(s))", sweeps.len()),
        Analysis::Ac {
            variation,
            points,
            fstart,
            fstop,
            ..
        } => format!(".ac {variation:?} {points} {fstart:.3e} {fstop:.3e}"),
        Analysis::Tran { tstep, tstop, .. } => format!(".tran {tstep:.3e} {tstop:.3e}"),
        Analysis::Noise {
            output,
            fstart,
            fstop,
            ..
        } => format!(".noise {output} {fstart:.3e} {fstop:.3e}"),
        other => format!("{other:?}"),
    }
}

fn draw_component_distribution(pixmap: &mut Pixmap, doc: &SpiceNetlistDocument) {
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let bar_count = doc.category_counts.len().clamp(1, 6);
    let max_val = doc
        .category_counts
        .iter()
        .map(|(_, c)| *c)
        .max()
        .unwrap_or(1)
        .max(1) as f32;

    let left = w * 0.08;
    let avail_w = w * 0.84;
    let bar_h = (h * 0.68 / bar_count as f32).clamp(4.0, 18.0);
    for idx in 0..doc.category_counts.len().min(6) {
        let count = doc.category_counts[idx].1 as f32;
        let bw = (avail_w * (count / max_val)).max(6.0);
        let y = h * 0.16 + idx as f32 * (bar_h + 3.0);
        let [r, g, b] = TRACE_PALETTE[idx % TRACE_PALETTE.len()];
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, 230);
        if let Some(rect) = Rect::from_xywh(left, y, bw, (bar_h * 0.72).max(3.0)) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }
}

struct RawSweepLayout<'a> {
    kind: RawAnalysisKind,
    complex: bool,
    stepped: bool,
    dimensions: Option<&'a [usize]>,
}

impl RawSweepLayout<'_> {
    fn has_axis(&self) -> bool {
        self.kind != RawAnalysisKind::DcOperatingPoint || self.complex
    }

    fn trace_indices(&self, nvars: usize) -> Range<usize> {
        let first = usize::from(self.has_axis() && nvars != 0);
        first..nvars.min(first + MAX_DECODED_TRACES)
    }

    fn ranges(
        &self,
        axis: impl ExactSizeIterator<Item = f64>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Range<usize>>, String> {
        sweeps::source_sweeps(axis, self.dimensions, self.kind, self.stepped, cancelled)
    }
}

fn convert_dune_rawfile(raw: Rawfile, cancelled: &AtomicBool) -> Result<SpiceRawDocument, String> {
    // The decoder only exposes unknown flags for the entire file. A stepped
    // warning can be matched to a specific plot when there is only one.
    let stepped = raw.plots.len() == 1
        && raw
            .warnings
            .iter()
            .any(|warning| warning.eq_ignore_ascii_case("unknown flag stepped"));
    let mut plots = Vec::new();
    for (p_idx, plot) in raw.plots.into_iter().take(MAX_RAW_PLOTS).enumerate() {
        if p_idx.is_multiple_of(2) {
            check_cancelled(cancelled, "SPICE raw conversion cancelled")?;
        }
        plots.push(convert_single_dune_plot(plot, stepped, cancelled)?);
    }
    let title = plots
        .first()
        .map(|p| p.title.clone())
        .unwrap_or_else(|| "SPICE Rawfile".into());
    Ok(SpiceRawDocument {
        title,
        plots,
        warnings: raw.warnings,
    })
}

fn convert_single_dune_plot(
    plot: dune_raw::Plot,
    stepped: bool,
    cancelled: &AtomicBool,
) -> Result<SpiceRawPlot, String> {
    let total_points = plot.num_points();
    let total_vars = plot.variables.len();
    let is_complex = plot.is_complex();
    let layout = RawSweepLayout {
        kind: RawAnalysisKind::classify(&plot.name, is_complex, total_points),
        complex: is_complex,
        stepped,
        dimensions: plot
            .scale()
            .and_then(|var| var.dims.as_deref())
            .or(plot.dimensions.as_deref()),
    };
    let traces = layout
        .trace_indices(total_vars)
        .map(|index| {
            let var = &plot.variables[index];
            SpiceRawTrace::new(var.name.clone(), var.vtype.name().to_string())
        })
        .collect();
    let preview = match &plot.data {
        dune_raw::Data::Real(columns) => {
            reduce_dune_columns(columns, &layout, traces, |x| (x, 0.0), cancelled)
        }
        dune_raw::Data::Complex(columns) => {
            reduce_dune_columns(columns, &layout, traces, |x| (x.re, x.im), cancelled)
        }
    }?;
    let kind = layout.kind;
    let has_axis = layout.has_axis();
    let scale = plot.scale().filter(|_| has_axis);
    let x_name = scale.map_or_else(String::new, |var| var.name.clone());
    let x_unit = scale.map_or_else(String::new, |var| var.vtype.name().to_string());
    Ok(SpiceRawPlot {
        title: if plot.title.trim().is_empty() {
            "SPICE Simulation".into()
        } else {
            plot.title
        },
        date: plot.date,
        plot_name: plot.name,
        kind,
        is_complex,
        total_points,
        total_vars,
        x_name,
        x_unit,
        x_range: preview.x_range.filter(|_| has_axis),
        traces: preview.traces,
    })
}

fn reduce_dune_columns<T: Copy>(
    columns: &[Vec<T>],
    layout: &RawSweepLayout<'_>,
    traces: Vec<SpiceRawTrace>,
    parts: impl Fn(T) -> (f64, f64),
    cancelled: &AtomicBool,
) -> Result<WaveformPreview, String> {
    let axis = columns.first().map_or(&[][..], Vec::as_slice);
    let signals = &columns[layout.trace_indices(columns.len())];
    let ranges = layout.ranges(axis.iter().map(|&sample| parts(sample).0), cancelled)?;
    peaks::reduce(
        &ranges,
        traces,
        layout.complex,
        |index, row| {
            for (signal, column) in row.iter_mut().zip(signals) {
                *signal = column.get(index).map(|&sample| {
                    let (re, im) = parts(sample);
                    displayed_value(re, im, layout.complex)
                });
            }
            Ok(if layout.has_axis() {
                parts(axis[index]).0
            } else {
                0.0
            })
        },
        cancelled,
    )
}

fn advance_brace_depth(mut depth: usize, line: &str) -> usize {
    for ch in line.split('"').step_by(2).flat_map(str::chars) {
        match ch {
            '{' => depth = depth.saturating_add(1),
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

#[derive(Default)]
struct RawHeaderMeta {
    title: String,
    date: String,
    plot_name: String,
    is_complex: bool,
    is_double: bool,
    fast_access: bool,
    stepped: bool,
    dimensions: Option<Vec<usize>>,
    nvars: usize,
    npoints: usize,
    variables: Vec<(String, String)>,
}

impl RawHeaderMeta {
    fn layout(&self) -> RawSweepLayout<'_> {
        RawSweepLayout {
            kind: RawAnalysisKind::classify(&self.plot_name, self.is_complex, self.npoints),
            complex: self.is_complex,
            stepped: self.stepped,
            dimensions: self.dimensions.as_deref(),
        }
    }

    fn traces(&self, layout: &RawSweepLayout<'_>) -> Vec<SpiceRawTrace> {
        self.variables[layout.trace_indices(self.nvars)]
            .iter()
            .map(|(name, unit)| SpiceRawTrace::new(name.clone(), unit.clone()))
            .collect()
    }
}

fn find_raw_data_marker(bytes: &[u8]) -> Option<(bool, usize)> {
    let markers: &[(&[u8], bool)] = &[
        (b"B\0i\0n\0a\0r\0y\0:\0\n\0", true),
        (b"B\0i\0n\0a\0r\0y\0:\0\r\0\n\0", true),
        (b"V\0a\0l\0u\0e\0s\0:\0\n\0", false),
        (b"V\0a\0l\0u\0e\0s\0:\0\r\0\n\0", false),
        (b"Binary:\n", true),
        (b"Binary:\r\n", true),
        (b"Values:\n", false),
        (b"Values:\r\n", false),
    ];
    markers
        .iter()
        .filter_map(|(pat, is_bin)| {
            bytes
                .windows(pat.len())
                .enumerate()
                .find(|&(pos, window)| {
                    window == *pat
                        && (bytes[..pos].ends_with(b"\n") || bytes[..pos].ends_with(b"\n\0"))
                })
                .map(|(pos, _)| (*is_bin, pos, pos + pat.len()))
        })
        .min_by_key(|(_, pos, _)| *pos)
        .map(|(is_bin, _, end)| (is_bin, end))
}

fn parse_ltspice_header(header_bytes: &[u8]) -> Result<RawHeaderMeta, String> {
    let text = decode_spice_text(header_bytes);
    let mut meta = RawHeaderMeta::default();
    let mut in_vars = false;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if in_vars {
            if let Some((name, unit)) = parse_variable_line(trimmed) {
                meta.variables.push((name, unit));
                if meta.variables.len() >= meta.nvars && meta.nvars > 0 {
                    in_vars = false;
                }
                continue;
            }
            in_vars = false;
        }
        apply_raw_header_line(trimmed, &mut meta, &mut in_vars);
    }
    if meta.nvars == 0 || meta.variables.len() != meta.nvars {
        return Err("SPICE .raw variable count does not match the header".into());
    }
    if meta.npoints == 0 {
        return Err("SPICE .raw header declares no data points".into());
    }
    Ok(meta)
}

fn parse_variable_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.split_whitespace();
    let _idx = parts.next()?.parse::<usize>().ok()?;
    let name = parts.next()?.to_string();
    let unit = parts.next().unwrap_or("").to_string();
    Some((name, unit))
}

fn apply_raw_header_line(line: &str, meta: &mut RawHeaderMeta, in_vars: &mut bool) {
    let lower = line.to_ascii_lowercase();
    if let Some(v) = line.split_once(':').map(|(_, r)| r.trim()) {
        if lower.starts_with("title:") {
            meta.title = v.to_string();
        } else if lower.starts_with("date:") {
            meta.date = v.to_string();
        } else if lower.starts_with("plotname:") {
            meta.plot_name = v.to_string();
        } else if lower.starts_with("flags:") {
            let fl = v.to_ascii_lowercase();
            meta.is_complex = fl.split_ascii_whitespace().any(|flag| flag == "complex");
            meta.is_double = fl.split_ascii_whitespace().any(|flag| flag == "double");
            meta.fast_access = fl.split_ascii_whitespace().any(|flag| flag == "fastaccess");
            meta.stepped = fl.split_ascii_whitespace().any(|flag| flag == "stepped");
        } else if lower.starts_with("no. variables:") {
            meta.nvars = v.parse().unwrap_or(0);
        } else if lower.starts_with("no. points:") {
            meta.npoints = v.parse().unwrap_or(0);
        } else if lower.starts_with("dimensions:") {
            meta.dimensions = v.split(',').map(|dim| dim.trim().parse().ok()).collect();
        } else if lower.starts_with("variables:") {
            *in_vars = true;
        }
    }
}

fn parse_ltspice_raw(bytes: &[u8], cancelled: &AtomicBool) -> Result<SpiceRawDocument, String> {
    let (is_binary, payload_offset) = find_raw_data_marker(bytes)
        .ok_or_else(|| "Missing Binary:/Values: section in SPICE .raw file".to_string())?;
    let meta = parse_ltspice_header(&bytes[..payload_offset])?;
    if meta.fast_access && !is_binary {
        return Err("LTspice FastAccess requires binary data".into());
    }

    let payload = &bytes[payload_offset..];
    let plot = if is_binary {
        decode_ltspice_binary_plot(&meta, payload, cancelled)?
    } else {
        decode_ltspice_ascii_plot(&meta, payload, cancelled)?
    };
    let title = plot.title.clone();
    Ok(SpiceRawDocument {
        title,
        plots: vec![plot],
        warnings: Vec::new(),
    })
}

fn decode_ltspice_binary_plot(
    meta: &RawHeaderMeta,
    payload: &[u8],
    cancelled: &AtomicBool,
) -> Result<SpiceRawPlot, String> {
    let nvars = meta.variables.len();
    let value_bytes = if meta.is_complex {
        16
    } else if meta.is_double {
        8
    } else {
        4
    };
    let first_bytes = value_bytes.max(8);
    let stride_bytes = (nvars - 1)
        .checked_mul(value_bytes)
        .and_then(|size| size.checked_add(first_bytes))
        .ok_or("Invalid LTspice variable count")?;
    let total_points = meta.npoints;
    if stride_bytes.checked_mul(total_points) != Some(payload.len()) {
        return Err("LTspice binary data size does not match the header".into());
    }
    let layout = meta.layout();
    let signals = layout.trace_indices(nvars);
    let has_axis = layout.has_axis();
    let sample = |row: usize, column: usize| {
        let (offset, size) = if column == 0 {
            (0, first_bytes)
        } else {
            (first_bytes + (column - 1) * value_bytes, value_bytes)
        };
        // The declared row size and total payload length have been checked.
        // FastAccess stores each variable's samples in its own contiguous block.
        let start = if meta.fast_access {
            offset * total_points + row * size
        } else {
            row * stride_bytes + offset
        };
        &payload[start..start + size]
    };
    let read_x = |row| {
        let x = binary_double(sample(row, 0));
        // LTspice's compressed transient format can encode negative time stamps.
        if layout.kind == RawAnalysisKind::Transient {
            x.abs()
        } else {
            x
        }
    };
    let ranges = layout.ranges((0..total_points).map(read_x), cancelled)?;
    let preview = peaks::reduce(
        &ranges,
        meta.traces(&layout),
        meta.is_complex,
        |point, row| {
            for (field, index) in row.iter_mut().zip(signals.clone()) {
                *field = Some(ltspice_binary_value(sample(point, index)));
            }
            Ok(if has_axis { read_x(point) } else { 0.0 })
        },
        cancelled,
    )?;
    Ok(build_ltspice_plot(meta, &layout, preview))
}

fn binary_double(bytes: &[u8]) -> f64 {
    f64::from_le_bytes(bytes.first_chunk::<8>().copied().unwrap_or_default())
}

fn ltspice_binary_value(bytes: &[u8]) -> peaks::Value {
    match bytes.len() {
        16 => displayed_value(binary_double(bytes), binary_double(&bytes[8..]), true),
        8 => (binary_double(bytes), 0.0),
        _ => (
            f64::from(f32::from_le_bytes(
                bytes.first_chunk::<4>().copied().unwrap_or_default(),
            )),
            0.0,
        ),
    }
}

fn decode_ltspice_ascii_plot(
    meta: &RawHeaderMeta,
    payload: &[u8],
    cancelled: &AtomicBool,
) -> Result<SpiceRawPlot, String> {
    let text = decode_spice_text(payload);
    // The sweep scan uses the declared row count. Even one byte per
    // field must fit in the decoded text before that scan is admitted.
    if meta.npoints > text.len() / meta.nvars.saturating_add(1) {
        return Err("SPICE ASCII data size does not match the header".into());
    }
    let mut tokens = text.split_whitespace();
    let mut axes = tokens.clone();
    let axis = (0..meta.npoints).map(|_| {
        axes.next(); // point index
        let x = parse_raw_scalar(axes.next(), meta.is_complex).map_or(f64::NAN, |(x, _)| x);
        if meta.nvars > 1 {
            axes.nth(meta.nvars - 2);
        }
        x
    });
    let layout = meta.layout();
    let ranges = layout.ranges(axis, cancelled)?;
    let first_signal = layout.trace_indices(meta.nvars).start;
    let preview = peaks::reduce(
        &ranges,
        meta.traces(&layout),
        meta.is_complex,
        |point, row| {
            if tokens.next().and_then(|token| token.parse::<usize>().ok()) != Some(point) {
                return Err("Invalid or missing SPICE .raw point index".into());
            }
            let x = if layout.has_axis() {
                parse_raw_scalar(tokens.next(), meta.is_complex)?.0
            } else {
                0.0
            };
            for var in first_signal..meta.nvars {
                let (re, im) = parse_raw_scalar(tokens.next(), meta.is_complex)?;
                if let Some(field) = row.get_mut(var - first_signal) {
                    *field = Some(displayed_value(re, im, meta.is_complex));
                }
            }
            Ok(x)
        },
        cancelled,
    )?;
    if tokens.next().is_some() {
        return Err("Unexpected data after SPICE .raw values".into());
    }
    Ok(build_ltspice_plot(meta, &layout, preview))
}

fn parse_raw_scalar(token: Option<&str>, complex: bool) -> Result<(f64, f64), String> {
    let invalid = || "Invalid or missing SPICE .raw sample".to_string();
    let token = token.ok_or_else(invalid)?;
    let parse = |value: &str| value.parse::<f64>().map_err(|_| invalid());
    if complex {
        let (re, im) = token.split_once(',').ok_or_else(invalid)?;
        Ok((parse(re)?, parse(im)?))
    } else {
        Ok((parse(token)?, 0.0))
    }
}

fn build_ltspice_plot(
    meta: &RawHeaderMeta,
    layout: &RawSweepLayout<'_>,
    preview: WaveformPreview,
) -> SpiceRawPlot {
    let has_axis = layout.has_axis();
    let (x_name, x_unit) = meta
        .variables
        .first()
        .filter(|_| has_axis)
        .cloned()
        .unwrap_or_default();
    SpiceRawPlot {
        title: if meta.title.trim().is_empty() {
            "LTspice Simulation".into()
        } else {
            meta.title.clone()
        },
        date: meta.date.clone(),
        plot_name: meta.plot_name.clone(),
        kind: layout.kind,
        is_complex: meta.is_complex,
        total_points: meta.npoints,
        total_vars: meta.variables.len(),
        x_name,
        x_unit,
        x_range: preview.x_range.filter(|_| has_axis),
        traces: preview.traces,
    }
}

fn draw_scope_grid(pixmap: &mut Pixmap) {
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let mut grid = Paint::default();
    grid.set_color_rgba8(44, 52, 66, 200);
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    let mut pb = PathBuilder::new();
    for i in 1..4 {
        let gx = w * (i as f32 / 4.0);
        let gy = h * (i as f32 / 4.0);
        pb.move_to(gx, 0.0);
        pb.line_to(gx, h);
        pb.move_to(0.0, gy);
        pb.line_to(w, gy);
    }
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &grid, &stroke, Transform::identity(), None);
    }
}

fn draw_operating_point_bars(pixmap: &mut Pixmap, plot: &SpiceRawPlot) {
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let count = plot.traces.len().clamp(1, 8);
    let vals: Vec<f64> = plot
        .traces
        .iter()
        .take(count)
        .map(|t| t.samples().first().map_or(0.0, |sample| sample.value.abs()))
        .collect();
    let max_v = vals.iter().copied().fold(1e-6_f64, f64::max);
    let row_h = (h * 0.72 / count as f32).clamp(3.0, 16.0);
    for (idx, val) in vals.into_iter().enumerate() {
        let bw = ((w * 0.78) * (val / max_v) as f32).max(4.0);
        let y = h * 0.14 + idx as f32 * (row_h + 2.0);
        let [r, g, b] = TRACE_PALETTE[idx % TRACE_PALETTE.len()];
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, 230);
        if let Some(rect) = Rect::from_xywh(w * 0.1, y, bw, (row_h * 0.72).max(2.5)) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }
}

fn draw_waveform_curves(
    pixmap: &mut Pixmap,
    plot: &SpiceRawPlot,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let Some([xmin, xmax]) = plot.x_range else {
        return Ok(());
    };
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let pad = 6.0_f32;
    let uw = (w - 2.0 * pad).max(4.0);
    let uh = (h - 2.0 * pad).max(4.0);

    let log_x = plot.log_x();
    let scale_x = |x: f64| if log_x { x.log10() } else { x };
    let [xmin, xmax] = [xmin, xmax].map(scale_x);
    let xspan = (xmax - xmin).max(1e-12);

    let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
    for t in plot.traces.iter().take(6) {
        if t.min.is_finite() {
            ymin = ymin.min(t.min);
        }
        if t.max.is_finite() {
            ymax = ymax.max(t.max);
        }
    }
    if !ymin.is_finite() || !ymax.is_finite() {
        return Ok(());
    }
    let yspan = (ymax - ymin).max(1e-9);

    for (t_idx, trace) in plot.traces.iter().take(6).enumerate() {
        check_cancelled(cancelled, "Waveform curve render cancelled")?;
        let [r, g, b] = TRACE_PALETTE[t_idx % TRACE_PALETTE.len()];
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, 235);
        paint.anti_alias = true;
        let mut pb = PathBuilder::new();
        let mut dots = PathBuilder::new();
        let point = |sample: &SpicePreviewSample| {
            [
                pad + ((scale_x(sample.x) - xmin) / xspan) as f32 * uw,
                pad + (1.0 - (sample.value - ymin) / yspan) as f32 * uh,
            ]
        };
        for run in trace
            .sweeps()
            .flat_map(|sweep| sweep.chunk_by(SpicePreviewSample::shares_run_with))
        {
            let Some((first, rest)) = run.split_first() else {
                continue;
            };
            let [px, py] = point(first);
            if rest.is_empty() {
                dots.push_circle(px, py, 0.9);
            } else {
                pb.move_to(px, py);
                for sample in rest {
                    let [px, py] = point(sample);
                    pb.line_to(px, py);
                }
            }
        }
        if let Some(path) = dots.finish() {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: 1.5,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    Ok(())
}

fn strip_key_eq_or_colon<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let lower = line.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix(key) {
        let rest_bytes = rest.trim_start();
        if rest_bytes.starts_with(':') || rest_bytes.starts_with('=') {
            let offset = line.len() - rest_bytes.len() + 1;
            return Some(line[offset..].trim());
        }
    }
    None
}

fn extract_ltspice_pane(line: &str) -> Option<LtspicePane> {
    let mut traces = Vec::new();
    let lower = line.to_ascii_lowercase();
    if let Some(idx) = lower.find("traces:") {
        traces = extract_quoted_or_csv_traces(&line[idx + "traces:".len()..]);
    }
    let log_x = lower.contains("log_x: 1") || lower.contains("log: 1");
    let log_y = lower.contains("log_y: 1");
    (!traces.is_empty()).then_some(LtspicePane {
        log_x,
        log_y,
        traces,
    })
}

fn apply_pane_line(pane: &mut LtspicePane, line: &str) {
    if let Some(rest) = strip_key_eq_or_colon(line, "log") {
        let mut flags = rest
            .split_whitespace()
            .map(|token| token.parse::<i32>().is_ok_and(|flag| flag != 0));
        pane.log_x = flags.next().unwrap_or(false);
        pane.log_y = flags.next().unwrap_or(false);
    } else if let Some(rest) = strip_key_eq_or_colon(line, "traces") {
        pane.traces.extend(extract_quoted_or_csv_traces(rest));
    }
}

fn extract_quoted_or_csv_traces(raw: &str) -> Vec<String> {
    // Native LTspice entries start with a count and store quoted trace names.
    // An empty native pane still has a count and braces.
    let count = raw
        .trim_start()
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '{')
        .next()
        .and_then(|token| token.parse::<usize>().ok());
    let mut out = Vec::new();
    let mut rest = raw;
    while out.len() < count.unwrap_or(usize::MAX)
        && let Some(start) = rest.find('"')
    {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else {
            break;
        };
        let trace = after[..end].trim();
        if !trace.is_empty() {
            out.push(trace.to_string());
        }
        rest = &after[end + 1..];
    }
    if out.is_empty() && count.is_none() {
        for tok in raw.trim_matches(['{', '}']).split(',') {
            let clean = tok.trim();
            if !clean.is_empty() {
                out.push(clean.to_string());
            }
        }
    }
    out
}

fn draw_ltspice_plt_panes(pixmap: &mut Pixmap, doc: &LtspicePlotDocument) {
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let Some(section) = doc
        .sections
        .iter()
        .find(|section| !section.panes.is_empty())
    else {
        return;
    };
    let count = section.panes.len().min(4);
    for (index, pane) in section.panes.iter().take(count).enumerate() {
        let height = h / count as f32;
        let top = index as f32 * height;
        let mut paint = Paint::default();
        paint.set_color_rgba8(38, 46, 58, 255);
        if let Some(box_rect) = Rect::from_xywh(3.0, top + 2.0, w - 6.0, height - 4.0) {
            pixmap.fill_rect(box_rect, &paint, Transform::identity(), None);
        }
        for (trace, &[r, g, b]) in TRACE_PALETTE.iter().take(pane.traces.len()).enumerate() {
            paint.set_color_rgba8(r, g, b, 230);
            if let Some(swatch) = Rect::from_xywh(8.0 + trace as f32 * 6.0, top + 5.0, 4.0, 4.0) {
                pixmap.fill_rect(swatch, &paint, Transform::identity(), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn preview_axis(trace: &SpiceRawTrace) -> Vec<f64> {
        trace.samples().iter().map(|sample| sample.x).collect()
    }

    fn sweep_lengths(trace: &SpiceRawTrace) -> Vec<usize> {
        trace.sweeps().map(<[_]>::len).collect()
    }

    #[test]
    fn netlist_totals_outlive_capped_preview_lists() {
        let mut deck = String::from("* Large resistor network\n");
        let count = MAX_NETLIST_NODES + 1;
        for index in 0..count {
            writeln!(deck, "R{index} node{index} 0 1000").unwrap();
        }
        deck.push_str("Ralias NODE1 0 1000\n.op\n.end\n");
        let doc = SpiceNetlistDocument::parse(
            Path::new("network.cir"),
            deck.as_bytes(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(doc.component_count, count + 1);
        assert_eq!(doc.node_count, count + 1);
        assert_eq!(doc.components.len(), MAX_NETLIST_LIST);
        assert_eq!(doc.nodes.len(), MAX_NETLIST_NODES);
        assert_eq!(doc.category_counts, [("Resistors", count + 1)]);
    }

    #[test]
    fn spice_netlist_parses_subcircuits_nodes_components_and_blocks_external_includes() {
        let cir = "\
* RC Lowpass Filter Benchmark
.include external_models/missing_lib.lib
.subckt BUF in out vdd
R1 in out 1k
C1 out 0 10n
.ends
.model DMOD D(IS=1e-14)
V1 in 0 AC 1 SIN(0 5 1k)
Rtop in mid 4.7k
Cbot mid 0 100n
X1 mid out vdd BUF
.tran 1u 5m
.ac dec 20 10 1Meg
.end
";
        let cancel = AtomicBool::new(false);
        let doc =
            SpiceNetlistDocument::parse(Path::new("rc.cir"), cir.as_bytes(), &cancel).unwrap();
        assert_eq!(doc.subcircuits.len(), 1);
        assert_eq!(doc.subcircuits[0].name, "BUF");
        assert!(doc.node_count >= 3);
        assert!(doc.component_count >= 4);
        assert!(doc.models.iter().any(|m| m.name == "DMOD"));
        assert!(doc.analyses.iter().any(|a| a.starts_with(".tran")));
        assert!(
            doc.includes
                .iter()
                .any(|inc| inc.contains("missing_lib.lib"))
        );

        let ng = "\
* ======================================
.title SR Latch Deck
Vdd vdd 0 1.8
XL1 a b q 0 SR
.tran 1n 10n
.control
set xgridwidth = 2
foreach x 1 2 3 4
  run
end
.endc
.end
";
        let ng_doc =
            SpiceNetlistDocument::parse(Path::new("ng.cir"), ng.as_bytes(), &cancel).unwrap();
        assert_eq!(ng_doc.title, "SR Latch Deck");
        assert!(ng_doc.node_count <= 5);

        let thumb = doc.render_thumbnail([180, 100], &cancel).unwrap();
        assert!(
            thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [SCOPE_BG[0], SCOPE_BG[1], SCOPE_BG[2]])
        );
    }

    #[test]
    fn spice_raw_parses_ascii_ac_complex_and_ltspice_utf16_binary_and_rejects_camera_raw() {
        let cancel = AtomicBool::new(false);

        let ascii_ac = "\
Title: * RC AC Sweep
Date: Tue Nov 24 10:00:00 2026
Plotname: AC Analysis
Flags: complex
No. Variables: 2
No. Points: 3
Variables:
\t0\tfrequency\tfrequency
\t1\tV(out)\tvoltage
Values:
 0\t1.000000e+01,0.000000e+00
\t9.990000e-01,-1.000000e-02
 1\t1.000000e+03,0.000000e+00
\t5.000000e-01,-5.000000e-01
 2\t1.000000e+05,0.000000e+00
\t1.000000e-02,-1.000000e-01
";
        assert!(is_spice_raw_bytes(ascii_ac.as_bytes()));
        let ac_doc = SpiceRawDocument::parse(ascii_ac.as_bytes(), &cancel).unwrap();
        let ac_plot = ac_doc.primary().unwrap();
        assert_eq!(ac_plot.kind, RawAnalysisKind::AcSweep);
        assert!(ac_plot.is_complex);
        assert_eq!(ac_plot.traces.len(), 1);
        assert!(ac_plot.traces[0].samples()[0].phase_deg < 0.0);
        let ac_thumb = ac_doc.render_thumbnail([180, 100], &cancel).unwrap();
        assert!(
            ac_thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [SCOPE_BG[0], SCOPE_BG[1], SCOPE_BG[2]])
        );

        // Build synthetic LTspice UTF-16LE f64-time + f32-value binary .raw file.
        let hdr = "\
Title: * LTspice Buck Converter
Date: Tue Nov 24 10:05:00 2026
Plotname: Transient Analysis
Flags: real forward
No. Variables: 2
No. Points: 4
Offset:   0.0000000000000000e+00
Variables:
\t0\ttime\ttime
\t1\tV(n001)\tvoltage
Binary:
";
        let mut lt_raw: Vec<u8> = vec![0xFF, 0xFE];
        for unit in hdr.encode_utf16() {
            lt_raw.extend_from_slice(&unit.to_le_bytes());
        }
        for i in 0..4 {
            let t = i as f64 * 1e-4;
            let v = (i as f32) * 1.25;
            lt_raw.extend_from_slice(&t.to_le_bytes());
            lt_raw.extend_from_slice(&v.to_le_bytes());
        }
        assert!(is_spice_raw_bytes(&lt_raw));
        let lt_doc = SpiceRawDocument::parse(&lt_raw, &cancel).unwrap();
        let lt_plot = lt_doc.primary().unwrap();
        assert_eq!(lt_plot.kind, RawAnalysisKind::Transient);
        assert_eq!(lt_plot.total_points, 4);
        assert!((lt_plot.traces[0].samples()[3].value - 3.75).abs() < 1e-4);

        let fake_tiff = b"II*\0\x08\0\0\0\x01\0\x00\x01\x03\0\x01\0\0\0";
        assert!(!is_spice_raw_bytes(fake_tiff));
    }

    #[test]
    fn descending_nested_dc_sweeps_draw_without_a_connector() {
        // Representative ngspice nested .dc output: descending inner source,
        // with two values of the outer source and no Dimensions header.
        let bytes = b"Title: Nested DC sweep\nPlotname: DC transfer characteristic\nFlags: real\nNo. Variables: 2\nNo. Points: 6\nVariables:\n0 v(v-sweep) voltage\n1 v(offset) voltage\nValues:\n0 1 -2\n1 0 -2\n2 -1 -2\n3 1 2\n4 0 2\n5 -1 2\n";
        let cancel = AtomicBool::new(false);
        let doc = SpiceRawDocument::parse(bytes, &cancel).unwrap();
        let plot = doc.primary().unwrap();
        assert_eq!(
            preview_axis(&plot.traces[0]),
            [1.0, 0.0, -1.0, 1.0, 0.0, -1.0]
        );
        assert_eq!(sweep_lengths(&plot.traces[0]), [3, 3]);
        let thumb = doc.render_thumbnail([128, 80], &cancel).unwrap();
        let green = |x: usize, y: usize| {
            let i = (y * 128 + x) * 4;
            let pixel: &[u8] = &thumb.premultiplied_rgba[i..i + 4];
            pixel[1] > 140
        };
        assert!(green(64, 6) && green(64, 74));
        assert!(!(62..67).any(|x| (38..43).any(|y| green(x, y))));
    }

    #[test]
    fn explicit_dimensions_separate_sweeps_even_without_an_axis_reset() {
        let header = "Title: Explicit sweeps\nPlotname: DC transfer characteristic\nFlags: real\nNo. Variables: 2\nNo. Points: 6\nDimensions: 2,3\n";
        let samples = "Variables:\n0 v-sweep voltage\n1 v(out) voltage\nValues:\n0 -3 0\n1 -2 0\n2 -1 0\n3 0 1\n4 1 1\n5 2 1\n";
        for extra in ["", "Offset: 0\n"] {
            // Offset exercises the LTspice fallback.
            let input = format!("{header}{extra}{samples}");
            let doc = SpiceRawDocument::parse(input.as_bytes(), &AtomicBool::new(false)).unwrap();
            assert_eq!(sweep_lengths(&doc.primary().unwrap().traces[0]), [3, 3]);
        }
    }

    #[test]
    fn peak_reduction_keeps_narrow_signed_excursions_in_each_sweep() {
        const SWEEP_POINTS: usize = 8192;
        let header = "Title: Narrow pulses\nPlotname: DC transfer characteristic\nFlags: real\nNo. Variables: 3\nNo. Points: 16384\nDimensions: 2,8192\n";
        let vars = "Variables:\n0 v-sweep voltage\n1 v(out) voltage\n2 v(second) voltage\n";
        let mut values = String::from("Values:\n");
        let mut binary = format!("{header}Offset: 0\n{vars}Binary:\n").into_bytes();
        for index in 0..2 * SWEEP_POINTS {
            let in_sweep = index % SWEEP_POINTS;
            let step = (index / SWEEP_POINTS) as f32;
            let x = in_sweep as f64 - 4096.0;
            // Indices 9 and 11 fell between the old stride-four samples.
            let y = match in_sweep {
                9 => 12.0 + step,
                11 => -21.0 - step,
                _ => 0.0,
            };
            let second = if in_sweep == 13 { -7.0 - step } else { 0.0 };
            writeln!(values, "{index} {x} {y} {second}").unwrap();
            binary.extend_from_slice(&x.to_le_bytes());
            binary.extend_from_slice(&y.to_le_bytes());
            binary.extend_from_slice(&second.to_le_bytes());
        }
        let ngspice = format!("{header}{vars}{values}");
        let ltspice_ascii = format!("{header}Offset: 0\n{vars}{values}");
        for bytes in [ngspice.as_bytes(), ltspice_ascii.as_bytes(), &binary] {
            let doc = SpiceRawDocument::parse(bytes, &AtomicBool::new(false)).unwrap();
            let plot = doc.primary().unwrap();
            assert_eq!(plot.x_range, Some([-4096.0, 4095.0]));
            for trace in &plot.traces {
                assert!(trace.samples().len() <= MAX_WAVEFORM_POINTS);
                assert_eq!(trace.sweeps().count(), 2);
                for (step, sweep) in trace.sweeps().enumerate() {
                    let start = step * SWEEP_POINTS;
                    assert_eq!(sweep.first().unwrap().source_index, start);
                    assert_eq!(sweep.last().unwrap().source_index, start + SWEEP_POINTS - 1);
                    assert!(
                        sweep
                            .windows(2)
                            .all(|pair| pair[0].source_index < pair[1].source_index)
                    );
                }
            }
            let main = &plot.traces[0];
            for (step, sweep) in main.sweeps().enumerate() {
                let spikes: Vec<_> = sweep
                    .iter()
                    .filter(|sample| sample.value != 0.0)
                    .map(|sample| (sample.x, sample.value))
                    .collect();
                assert_eq!(
                    spikes,
                    [
                        (-4087.0, 12.0 + step as f64),
                        (-4085.0, -21.0 - step as f64)
                    ]
                );
            }
            assert_eq!(main.min, -22.0);
            assert_eq!(main.max, 13.0);
            assert!(plot.traces[0].sample_at(13).is_none());
            assert_eq!(plot.traces[1].sample_at(13).unwrap().value, -7.0);
        }
    }

    #[test]
    fn complex_reduction_preserves_magnitude_and_phase_extrema() {
        let mut raw = String::from(
            "Title: AC peaks\nPlotname: AC Analysis\nFlags: complex\nNo. Variables: 2\nNo. Points: 8192\nVariables:\n0 frequency frequency\n1 v(out) voltage\nValues:\n",
        );
        for index in 0..8192 {
            let (re, im) = match index {
                3 => (10.0, 0.0),
                5 => (0.1, 0.0),
                7 => (0.0, -1.0),
                9 => (0.0, 1.0),
                _ => (1.0, 0.0),
            };
            writeln!(raw, "{index} {},0 {re},{im}", index + 1).unwrap();
        }
        let doc = SpiceRawDocument::parse(raw.as_bytes(), &AtomicBool::new(false)).unwrap();
        let trace = &doc.primary().unwrap().traces[0];
        assert!(trace.samples().len() <= MAX_WAVEFORM_POINTS);
        assert_eq!(trace.sample_at(3).unwrap().value, 20.0);
        assert_eq!(trace.sample_at(5).unwrap().value, -20.0);
        assert_eq!(trace.sample_at(7).unwrap().phase_deg, -90.0);
        assert_eq!(trace.sample_at(9).unwrap().phase_deg, 90.0);
    }

    #[test]
    fn an_ordinary_axis_reversal_is_preserved() {
        let bytes = b"Title: DC sweep\nPlotname: DC transfer characteristic\nFlags: real\nNo. Variables: 2\nNo. Points: 6\nVariables:\n0 v-sweep voltage\n1 v(out) voltage\nValues:\n0 -2 0\n1 1 1\n2 0 2\n3 -2 3\n4 1 4\n5 2 5\n";
        let doc = SpiceRawDocument::parse(bytes, &AtomicBool::new(false)).unwrap();
        let plot = doc.primary().unwrap();
        assert_eq!(sweep_lengths(&plot.traces[0]), [6]);
        assert_eq!(
            preview_axis(&plot.traces[0]),
            [-2.0, 1.0, 0.0, -2.0, 1.0, 2.0]
        );
    }

    #[test]
    fn fast_access_uses_column_offsets_even_when_record_sizes_match() {
        let mut bytes = b"Title: FastAccess demo\nPlotname: Transient Analysis\nFlags: real double FastAccess\nNo. Variables: 2\nNo. Points: 2\nVariables:\n0 time time\n1 V(out) voltage\nBinary:\n".to_vec();
        for column in [[0.0_f64, 1.0], [5.0, 7.0]] {
            for value in column {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let cancel = AtomicBool::new(false);
        let doc = SpiceRawDocument::parse(&bytes, &cancel).unwrap();
        let trace = &doc.primary().unwrap().traces[0];
        assert_eq!(preview_axis(trace), [0.0, 1.0]);
        assert_eq!(trace.sample_at(0).unwrap().value, 5.0);
        assert_eq!(trace.sample_at(1).unwrap().value, 7.0);
        assert!(
            SpiceRawDocument::parse(&bytes[..bytes.len() - 1], &cancel)
                .unwrap_err()
                .contains("data size")
        );
    }

    #[test]
    fn utf16_fast_access_mixed_precision_keeps_steps_and_negative_peaks() {
        let points_per_step = 3000;
        let header = format!(
            "Title: FastAccess run\r\nPlotname: Transient Analysis\r\nFlags: real forward stepped FastAccess\r\nNo. Variables: 2\r\nNo. Points: {}\r\nVariables:\r\n0 time time\r\n1 V(out) voltage\r\nBinary:\r\n",
            points_per_step * 2
        );
        let mut bytes: Vec<_> = header.encode_utf16().flat_map(u16::to_le_bytes).collect();
        for row in 0..points_per_step * 2 {
            let time = (row % points_per_step) as f64 * 0.00001;
            bytes.extend_from_slice(&time.to_le_bytes());
        }
        for row in 0..points_per_step * 2 {
            let value: f32 = if row == points_per_step + 5 {
                -18.0
            } else {
                1.0
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let doc = SpiceRawDocument::parse(&bytes, &AtomicBool::new(false)).unwrap();
        let trace = &doc.primary().unwrap().traces[0];
        assert_eq!(trace.sweeps().count(), 2);
        assert!(trace.samples().len() <= MAX_WAVEFORM_POINTS);
        let peak = trace.sample_at(points_per_step + 5).unwrap();
        assert_eq!(peak.value, -18.0);
        assert!((peak.x - 0.00005).abs() < 1e-12);
    }

    #[test]
    fn fast_access_complex_traces_keep_frequency_magnitude_and_phase() {
        let mut bytes = b"Title: FastAccess AC\nPlotname: AC Analysis\nFlags: complex FastAccess\nNo. Variables: 2\nNo. Points: 2\nVariables:\n0 frequency frequency\n1 V(out) voltage\nBinary:\n".to_vec();
        for value in [1000.0_f64, 0.0, 2000.0, 0.0, 1.0, 0.0, 0.0, -2.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let doc = SpiceRawDocument::parse(&bytes, &AtomicBool::new(false)).unwrap();
        let trace = &doc.primary().unwrap().traces[0];
        assert_eq!(preview_axis(trace), [1000.0, 2000.0]);
        let sample = trace.sample_at(1).unwrap();
        assert!((sample.value - 20.0 * 2_f64.log10()).abs() < 1e-12);
        assert_eq!(sample.phase_deg, -90.0);
    }

    #[test]
    fn raw_identification_requires_waveform_header_lines() {
        assert!(!is_spice_raw_bytes(
            b"Comparison document: Title: Demo, No. Variables: 2, Plotname: Sweep"
        ));
        assert!(is_spice_raw_bytes(
            b"TITLE: Demo\nPlotname: Sweep\nNo. Variables: 2\n"
        ));
        let raw = b"Title: Saved as Binary:\nPlotname: Sweep\nNo. Variables: 2\nValues:\n";
        assert_eq!(find_raw_data_marker(raw), Some((false, raw.len())));
    }

    #[test]
    fn operating_point_keeps_the_first_voltage_as_a_signal() {
        let raw = b"Title: Operating point\nPlotname: Operating Point\nFlags: real\nNo. Variables: 2\nNo. Points: 1\nVariables:\n0 V(in) voltage\n1 V(out) voltage\nValues:\n0 5 3.3\n";
        let doc = SpiceRawDocument::parse(raw, &AtomicBool::new(false)).unwrap();
        let plot = doc.primary().unwrap();
        assert_eq!(plot.kind, RawAnalysisKind::DcOperatingPoint);
        let values: Vec<_> = plot
            .traces
            .iter()
            .map(|t| (t.name.as_str(), t.samples()[0].value))
            .collect();
        assert_eq!(values, [("V(in)", 5.0), ("V(out)", 3.3)]);
        assert!(plot.x_range.is_none());
    }

    #[test]
    fn ngspice_binary_operating_point_preserves_signed_current_and_both_voltages() {
        // Header and row from ngspice 47 for a 4 V two-resistor divider.
        let mut raw = b"Title: * inspection fixture: two-resistor divider\nCommand: ngspice-47\nPlotname: Operating Point\nFlags: real\nNo. Variables: 3\nNo. Points: 1       \nVariables:\n\t0\tv(in)\tvoltage\n\t1\tv(out)\tvoltage\n\t2\ti(v1)\tcurrent\nBinary:\n".to_vec();
        for value in [4.0_f64, 2.0, -0.002] {
            raw.extend_from_slice(&value.to_ne_bytes());
        }
        let doc = SpiceRawDocument::parse(&raw, &AtomicBool::new(false)).unwrap();
        let plot = doc.primary().unwrap();
        let values: Vec<_> = plot
            .traces
            .iter()
            .map(|trace| (trace.name.as_str(), trace.samples()[0].value))
            .collect();
        assert_eq!(values, [("v(in)", 4.0), ("v(out)", 2.0), ("i(v1)", -0.002)]);
        assert!(plot.x_range.is_none());
    }

    #[test]
    fn dc_sweep_preserves_negative_axis_values() {
        let header = "Title: Sweep\nPlotname: DC transfer characteristic\nFlags: real\nNo. Variables: 2\nNo. Points: 3\nVariables:\n0 v-sweep voltage\n1 V(out) voltage\n";
        let cancel = AtomicBool::new(false);
        let ascii = format!("{header}Values:\n0 -1 -2\n1 0 0\n2 1 2\n");
        let doc = SpiceRawDocument::parse(ascii.as_bytes(), &cancel).unwrap();
        assert_eq!(
            preview_axis(&doc.primary().unwrap().traces[0]),
            [-1.0, 0.0, 1.0]
        );

        let mut binary = format!("{header}Binary:\n").into_bytes();
        for x in [-1.0_f64, 0.0, 1.0] {
            binary.extend_from_slice(&x.to_le_bytes());
            binary.extend_from_slice(&(2.0 * x as f32).to_le_bytes());
        }
        let doc = SpiceRawDocument::parse(&binary, &cancel).unwrap();
        assert_eq!(
            preview_axis(&doc.primary().unwrap().traces[0]),
            [-1.0, 0.0, 1.0]
        );
    }

    #[test]
    fn raw_preview_rejects_missing_and_invalid_ascii_samples() {
        let header = "Title: Incomplete\nPlotname: Transient Analysis\nFlags: real\nNo. Variables: 2\nNo. Points: 2\nVariables:\n0 time time\n1 V(out) voltage\nValues:\n";
        let cancel = AtomicBool::new(false);
        for values in ["0 0 5\n1 1", "0 0 5\n1 1 missing"] {
            let raw = format!("{header}{values}\n");
            assert!(SpiceRawDocument::parse(raw.as_bytes(), &cancel).is_err());
        }
    }

    #[test]
    fn ltspice_plt_settings_disambiguate_from_hpgl_plotter_files() {
        let lt_plt = "[Transient Analysis]\n{\n   Npanes: 2\n   {\n      traces: 2 { (524290,0,\"V(out)\") (524291,0,\"I(R1)\") }\n   }\n   {\n      traces: 1 { (524292,0,\"V(in)\") }\n   }\n}\n";
        assert!(looks_like_ltspice_plt(lt_plt.as_bytes()));
        let cancel = AtomicBool::new(false);
        let doc = LtspicePlotDocument::parse(lt_plt.as_bytes(), &cancel).unwrap();
        assert_eq!(doc.sections[0].analysis, "Transient Analysis");
        assert_eq!(doc.pane_count(), 2);
        assert_eq!(doc.trace_count, 3);

        let hpgl = "IN;SP1;PU0,0;PD1000,0,1000,1000;LB[AC Analysis]\x03;SP0;";
        assert!(!looks_like_ltspice_plt(hpgl.as_bytes()));
    }

    #[test]
    fn ltspice_settings_keep_panes_under_their_own_analysis() {
        let source = br#"[Transient Analysis]
{
  Npanes: 1
  {
    traces: 1 { (1,0,"V(out)") }
    Log: 0 0 0
  }
}
[AC Analysis]
{
  Npanes: 1
  {
    traces: 2 { (1,0,"V(gain)") (2,0,"I(V1)") }
    Log: 1 1 0
  }
}"#;
        let doc = LtspicePlotDocument::parse(source, &AtomicBool::new(false)).unwrap();
        assert_eq!(doc.pane_count(), 2);
        assert_eq!(doc.trace_count, 3);
        assert_eq!(doc.sections[0].analysis, "Transient Analysis");
        assert_eq!(doc.sections[0].panes[0].traces, ["V(out)"]);
        assert!(!doc.sections[0].panes[0].log_x);
        assert_eq!(doc.sections[1].analysis, "AC Analysis");
        assert_eq!(doc.sections[1].panes[0].traces, ["V(gain)", "I(V1)"]);
        assert!(doc.sections[1].panes[0].log_x);
        assert!(doc.summary_line().contains("2 analyses"));
    }

    #[test]
    fn ltspice_empty_panes_have_no_configured_trace_names() {
        let source = b"[Transient Analysis]\n{\nNpanes: 1\n{\ntraces: 0 { }\n}\n}\n";
        let doc = LtspicePlotDocument::parse(source, &AtomicBool::new(false)).unwrap();
        assert_eq!(doc.pane_count(), 1);
        assert_eq!(doc.trace_count, 0);
        assert!(doc.sections[0].panes[0].traces.is_empty());
        assert!(doc.strip_summary().specs.contains("No traces configured"));
    }
}
