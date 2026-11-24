//! SPICE circuit netlists (`.cir`, `.sp`, `.spice`), SPICE/LTspice simulation
//! output waveforms (`.raw`), and LTspice plot settings (`.plt`).

use super::{MAX_SOURCE_CHARS, StripSummary, check_cancelled, pixmap_to_thumbnail};
use crate::ImageThumbnail;
use dune_netlist::IncludeLoader;
use dune_netlist::ir::Analysis;
use dune_raw::Rawfile;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

const SCOPE_BG: [u8; 4] = [16, 20, 26, 255];
const MAX_NETLIST_LIST: usize = 2048;
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

        let scanned = scan_spice_cards(&text, cancelled)?;
        let scanned_node_count = scanned.nodes.len();
        let title = pick_netlist_title(&outcome.netlist.title, &scanned.title, path);

        let (components, category_counts, distinct_nodes) = if outcome.netlist.elements.is_empty() {
            (scanned.components, scanned.category_counts, scanned.nodes)
        } else {
            summarize_ir_elements(&outcome.netlist.elements)
        };
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
            node_count: distinct_nodes.len().max(scanned_node_count),
            nodes: distinct_nodes,
            component_count: components.len(),
            components,
            category_counts,
            subcircuits: scanned.subcircuits,
            models,
            analyses,
            includes: scanned.includes,
            diagnostics,
            source_excerpt: bounded_spice_text(&text),
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

        draw_netlist_graph_motif(&mut pixmap, self);
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
pub struct SpiceRawTrace {
    pub name: String,
    pub unit_type: String,
    pub values: Vec<f64>,
    pub phase_deg: Option<Vec<f64>>,
    pub min: f64,
    pub max: f64,
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
    pub x_values: Vec<f64>,
    pub traces: Vec<SpiceRawTrace>,
}

impl SpiceRawPlot {
    fn include_first_operating_point_signal(mut self) -> Self {
        if self.kind == RawAnalysisKind::DcOperatingPoint
            && !self.is_complex
            && let Some(&value) = self.x_values.first()
        {
            self.traces.truncate(MAX_DECODED_TRACES - 1);
            self.traces.insert(
                0,
                SpiceRawTrace {
                    name: std::mem::take(&mut self.x_name),
                    unit_type: std::mem::take(&mut self.x_unit),
                    min: value,
                    max: value,
                    values: std::mem::take(&mut self.x_values),
                    phase_deg: None,
                },
            );
        }
        self
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
        if !is_spice_raw_bytes(bytes) {
            return Err("File is not a SPICE/LTspice .raw simulation output".into());
        }
        if let Ok(raw) = Rawfile::parse(bytes)
            && !raw.plots.is_empty()
            && !raw.plots.iter().all(|p| p.data.num_points() == 0)
        {
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
        let x_span = match (p.x_values.first(), p.x_values.last()) {
            (Some(a), Some(b)) => format!("{} [{a:.3e}..{b:.3e}]", p.x_name),
            _ => p.x_name.clone(),
        };
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

#[derive(Clone, Debug)]
pub struct LtspicePane {
    pub index: usize,
    pub log_x: bool,
    pub log_y: bool,
    pub traces: Vec<String>,
}

/// Decoded LTspice `.plt` plot settings document.
#[derive(Clone, Debug)]
pub struct LtspicePlotDocument {
    pub analysis: String,
    pub panes: Vec<LtspicePane>,
    pub trace_count: usize,
    pub source_excerpt: String,
}

impl LtspicePlotDocument {
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "LTspice .plt parse cancelled")?;
        let text = decode_spice_text(bytes);
        let mut analysis = "LTspice Plot Settings".to_string();
        let mut pane_traces: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        let mut pane_logs: BTreeMap<usize, (bool, bool)> = BTreeMap::new();
        let mut current_pane = 0_usize;
        let mut next_pane = 0_usize;
        let mut brace_depth = 0_usize;

        for raw in text.lines().take(4096) {
            let line = raw.trim();
            if line.starts_with('[') && line.ends_with(']') {
                analysis = line.trim_matches(['[', ']']).to_string();
                brace_depth = 0;
                continue;
            }
            if line.starts_with('{') && brace_depth == 1 {
                current_pane = next_pane;
                next_pane += 1;
                pane_traces.entry(current_pane).or_default();
            }
            if let Some(rest) = strip_key_eq_or_colon(line, "log") {
                let vals: Vec<i32> = rest
                    .split_whitespace()
                    .filter_map(|t| t.parse::<i32>().ok())
                    .collect();
                let lx = vals.first().copied().unwrap_or(0) != 0;
                let ly = vals.get(1).copied().unwrap_or(0) != 0;
                pane_logs.insert(current_pane, (lx, ly));
            } else if let Some(rest) = strip_key_eq_or_colon(line, "traces") {
                let list = extract_quoted_or_csv_traces(rest);
                pane_traces.entry(current_pane).or_default().extend(list);
            } else if line.starts_with('{')
                && let Some(pane) = extract_ltspice_pane(line)
            {
                pane_traces
                    .entry(current_pane)
                    .or_default()
                    .extend(pane.traces);
                pane_logs
                    .entry(current_pane)
                    .or_insert((pane.log_x, pane.log_y));
            }
            brace_depth = advance_brace_depth(brace_depth, line);
        }

        let panes: Vec<LtspicePane> = pane_traces
            .into_iter()
            .map(|(index, traces)| {
                let (log_x, log_y) = pane_logs.get(&index).copied().unwrap_or((false, false));
                LtspicePane {
                    index,
                    log_x,
                    log_y,
                    traces,
                }
            })
            .collect();
        let trace_count: usize = panes.iter().map(|p| p.traces.len()).sum();

        Ok(Self {
            analysis,
            panes,
            trace_count,
            source_excerpt: bounded_spice_text(&text),
        })
    }

    pub fn summary_line(&self) -> String {
        format!(
            "LTspice Plot · {} · {} panes · {} traces",
            self.analysis,
            self.panes.len(),
            self.trace_count
        )
    }

    pub fn strip_summary(&self) -> StripSummary {
        let preview_traces: Vec<&str> = self
            .panes
            .iter()
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
            headline: format!("LTspice Plot · {}", self.analysis),
            specs: format!(
                "{} panes · {} traces · {list}",
                self.panes.len(),
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
    if bytes.len() < 16 || looks_like_camera_raw_magic(bytes) {
        return false;
    }
    let prefix = &bytes[..bytes.len().min(4096)];
    let decoded = decode_spice_text(prefix).to_ascii_lowercase();
    decoded.contains("title:")
        && (decoded.contains("plotname:")
            || decoded.contains("no. variables:")
            || decoded.contains("variables:")
            || decoded.contains("flags:"))
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
    let lower = decode_spice_text(prefix).to_ascii_lowercase();
    lower.contains("[transient analysis]")
        || lower.contains("[ac analysis]")
        || lower.contains("[dc sweep]")
        || lower.contains("[dc transfer characteristic]")
        || lower.contains("[noise analysis]")
        || lower.contains("[dc operating point]")
        || lower.contains("[fft]")
        || (lower.contains("npanes:") && lower.contains("traces:"))
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

fn bounded_spice_text(text: &str) -> String {
    if text.len() <= MAX_SOURCE_CHARS {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(MAX_SOURCE_CHARS);
        let mut out = String::with_capacity(end + 64);
        out.push_str(&text[..end]);
        out.push_str("\n… [source truncated for preview]");
        out
    }
}

struct ScannedNetlist {
    title: String,
    nodes: Vec<String>,
    components: Vec<SpiceComponentInfo>,
    category_counts: Vec<(&'static str, usize)>,
    subcircuits: Vec<SpiceSubcktInfo>,
    models: Vec<SpiceModelInfo>,
    analyses: Vec<String>,
    includes: Vec<String>,
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

fn scan_spice_cards(text: &str, cancelled: &AtomicBool) -> Result<ScannedNetlist, String> {
    let cards = join_logical_lines(text);
    let mut title = String::new();
    let mut dot_title = String::new();
    let mut node_set: BTreeSet<String> = BTreeSet::new();
    let mut components = Vec::new();
    let mut cats: BTreeMap<&'static str, usize> = BTreeMap::new();
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
        record_top_element(trimmed, &mut components, &mut cats, &mut node_set);
    }
    if let Some(sub) = active_subckt {
        subcircuits.push(sub);
    }
    if !dot_title.is_empty() {
        title = dot_title;
    }

    let category_counts: Vec<(&'static str, usize)> = cats.into_iter().collect();
    Ok(ScannedNetlist {
        title,
        nodes: node_set.into_iter().collect(),
        components,
        category_counts,
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

fn record_top_element(
    line: &str,
    components: &mut Vec<SpiceComponentInfo>,
    cats: &mut BTreeMap<&'static str, usize>,
    node_set: &mut BTreeSet<String>,
) {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let Some(name) = tokens.first().copied() else {
        return;
    };
    let Some(first_ch) = name.chars().next() else {
        return;
    };
    let (kind, cat_label, min_nodes) = spice_device_kind(first_ch);
    *cats.entry(cat_label).or_default() += 1;

    let end = if first_ch.eq_ignore_ascii_case(&'X') {
        tokens.len().saturating_sub(1).max(2)
    } else {
        (1 + min_nodes).min(tokens.len())
    };
    let nodes: Vec<String> = tokens
        .get(1..end)
        .unwrap_or(&[])
        .iter()
        .filter(|n| !n.contains('='))
        .map(|s| (*s).to_string())
        .collect();
    for n in &nodes {
        if node_set.len() < 4096 {
            node_set.insert(n.clone());
        }
    }
    if components.len() < MAX_NETLIST_LIST {
        components.push(SpiceComponentInfo {
            name: name.to_string(),
            kind,
            nodes,
        });
    }
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

fn summarize_ir_elements(
    elements: &[dune_netlist::ir::Element],
) -> (
    Vec<SpiceComponentInfo>,
    Vec<(&'static str, usize)>,
    Vec<String>,
) {
    let mut components = Vec::new();
    let mut cats: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut node_set: BTreeSet<String> = BTreeSet::new();
    for el in elements {
        let prefix = el.name.chars().next().unwrap_or('R');
        let (kind, cat, _) = spice_device_kind(prefix);
        *cats.entry(cat).or_default() += 1;
        for node in &el.nodes {
            if node_set.len() < 4096 {
                node_set.insert(node.clone());
            }
        }
        if components.len() < MAX_NETLIST_LIST {
            components.push(SpiceComponentInfo {
                name: el.name.clone(),
                kind,
                nodes: el.nodes.clone(),
            });
        }
    }
    (
        components,
        cats.into_iter().collect(),
        node_set.into_iter().collect(),
    )
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

fn draw_netlist_graph_motif(pixmap: &mut Pixmap, doc: &SpiceNetlistDocument) {
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
    let avail_w = w * 0.44;
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

    // Draw node connectivity ring motif on the right half.
    let cx = w * 0.75;
    let cy = h * 0.5;
    let rad = (w * 0.22).min(h * 0.36).max(12.0);
    let n_nodes = doc.node_count.clamp(3, 12);
    let pts: Vec<[f32; 2]> = (0..n_nodes)
        .map(|i| {
            let a = std::f32::consts::TAU * (i as f32 / n_nodes as f32);
            [cx + rad * a.cos(), cy + rad * a.sin()]
        })
        .collect();
    let mut edge_paint = Paint::default();
    edge_paint.set_color_rgba8(122, 162, 247, 130);
    edge_paint.anti_alias = true;
    for i in 0..pts.len() {
        let next = (i + 1) % pts.len();
        let opp = (i + pts.len() / 2) % pts.len();
        let mut pb = PathBuilder::new();
        pb.move_to(pts[i][0], pts[i][1]);
        pb.line_to(pts[next][0], pts[next][1]);
        pb.move_to(pts[i][0], pts[i][1]);
        pb.line_to(pts[opp][0], pts[opp][1]);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(
                &path,
                &edge_paint,
                &Stroke {
                    width: 1.2,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
    let mut node_paint = Paint::default();
    node_paint.set_color_rgba8(98, 224, 134, 240);
    node_paint.anti_alias = true;
    for [x, y] in pts {
        let mut pb = PathBuilder::new();
        pb.push_circle(x, y, 2.8);
        if let Some(path) = pb.finish() {
            pixmap.fill_path(
                &path,
                &node_paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

fn decimate_stride(total_points: usize) -> usize {
    total_points.div_ceil(MAX_WAVEFORM_POINTS).max(1)
}

fn min_max_finite(vals: &[f64]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &v in vals {
        if v.is_finite() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if lo.is_finite() && hi.is_finite() {
        (lo, hi)
    } else {
        (0.0, 1.0)
    }
}

fn convert_dune_rawfile(raw: Rawfile, cancelled: &AtomicBool) -> Result<SpiceRawDocument, String> {
    let mut plots = Vec::new();
    for (p_idx, plot) in raw.plots.into_iter().take(MAX_RAW_PLOTS).enumerate() {
        if p_idx.is_multiple_of(2) {
            check_cancelled(cancelled, "SPICE raw conversion cancelled")?;
        }
        plots.push(convert_single_dune_plot(plot));
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

fn convert_single_dune_plot(plot: dune_raw::Plot) -> SpiceRawPlot {
    let total_points = plot.data.num_points();
    let total_vars = plot.variables.len();
    let stride = decimate_stride(total_points);
    let is_complex = plot.is_complex();
    let kind = RawAnalysisKind::classify(&plot.name, is_complex, total_points);
    let x_var = plot.variables.first();
    let x_name = x_var.map_or_else(|| "x".into(), |v| v.name.clone());
    let x_unit = x_var.map_or_else(String::new, |v| v.vtype.name().to_string());

    let (x_values, traces) = match &plot.data {
        dune_raw::Data::Real(cols) => extract_real_traces(&plot.variables, cols, stride),
        dune_raw::Data::Complex(cols) => {
            let pairs: Vec<Vec<(f64, f64)>> = cols
                .iter()
                .take(MAX_DECODED_TRACES + 1)
                .map(|col| col.iter().step_by(stride).map(|c| (c.re, c.im)).collect())
                .collect();
            extract_complex_traces(&plot.variables, &pairs)
        }
    };

    SpiceRawPlot {
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
        x_values,
        traces,
    }
    .include_first_operating_point_signal()
}

fn extract_real_traces(
    vars: &[dune_raw::Variable],
    cols: &[Vec<f64>],
    stride: usize,
) -> (Vec<f64>, Vec<SpiceRawTrace>) {
    let x_values = cols
        .first()
        .map(|col| col.iter().step_by(stride).copied().collect())
        .unwrap_or_default();
    let mut traces = Vec::new();
    for (idx, col) in cols.iter().enumerate().skip(1).take(MAX_DECODED_TRACES) {
        let values: Vec<f64> = col.iter().step_by(stride).copied().collect();
        let (min, max) = min_max_finite(&values);
        let var = vars.get(idx);
        traces.push(SpiceRawTrace {
            name: var.map_or_else(|| format!("v{idx}"), |v| v.name.clone()),
            unit_type: var.map_or_else(String::new, |v| v.vtype.name().to_string()),
            values,
            phase_deg: None,
            min,
            max,
        });
    }
    (x_values, traces)
}

fn advance_brace_depth(mut depth: usize, line: &str) -> usize {
    for ch in line.chars() {
        match ch {
            '{' => depth = depth.saturating_add(1),
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn extract_complex_traces(
    vars: &[dune_raw::Variable],
    cols: &[Vec<(f64, f64)>],
) -> (Vec<f64>, Vec<SpiceRawTrace>) {
    let x_values = cols
        .first()
        .map(|col| {
            col.iter()
                .map(|&(re, im)| re.hypot(im).max(re.abs()))
                .collect()
        })
        .unwrap_or_default();
    let mut traces = Vec::new();
    for (idx, col) in cols.iter().enumerate().skip(1).take(MAX_DECODED_TRACES) {
        let mut mag_db = Vec::with_capacity(col.len());
        let mut phase_deg = Vec::with_capacity(col.len());
        for &(re, im) in col {
            let norm = re.hypot(im).max(1e-18);
            mag_db.push(20.0 * norm.log10());
            phase_deg.push(im.atan2(re).to_degrees());
        }
        let (min, max) = min_max_finite(&mag_db);
        let var = vars.get(idx);
        traces.push(SpiceRawTrace {
            name: var.map_or_else(|| format!("v{idx}"), |v| v.name.clone()),
            unit_type: var.map_or_else(String::new, |v| v.vtype.name().to_string()),
            values: mag_db,
            phase_deg: Some(phase_deg),
            min,
            max,
        });
    }
    (x_values, traces)
}

#[derive(Default)]
struct RawHeaderMeta {
    title: String,
    date: String,
    plot_name: String,
    is_complex: bool,
    is_double: bool,
    fast_access: bool,
    nvars: usize,
    npoints: usize,
    variables: Vec<(String, String)>,
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
                .position(|w| w == *pat)
                .map(|pos| (*is_bin, pos, pos + pat.len()))
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
        } else if lower.starts_with("no. variables:") {
            meta.nvars = v.parse().unwrap_or(0);
        } else if lower.starts_with("no. points:") {
            meta.npoints = v.parse().unwrap_or(0);
        } else if lower.starts_with("variables:") {
            *in_vars = true;
        }
    }
}

fn parse_ltspice_raw(bytes: &[u8], cancelled: &AtomicBool) -> Result<SpiceRawDocument, String> {
    let (is_binary, payload_offset) = find_raw_data_marker(bytes)
        .ok_or_else(|| "Missing Binary:/Values: section in SPICE .raw file".to_string())?;
    let meta = parse_ltspice_header(&bytes[..payload_offset])?;

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
    if meta.fast_access {
        return Err("LTspice FastAccess binary layout is unsupported".into());
    }
    let nvars = meta.variables.len();
    let stride_bytes = if meta.is_complex {
        nvars.checked_mul(16)
    } else if meta.is_double {
        nvars.checked_mul(8)
    } else {
        (nvars - 1)
            .checked_mul(4)
            .and_then(|size| size.checked_add(8))
    }
    .ok_or("Invalid LTspice variable count")?;
    let total_points = meta.npoints;
    if stride_bytes.checked_mul(total_points) != Some(payload.len()) {
        return Err("LTspice binary data size does not match the header".into());
    }
    let step = decimate_stride(total_points);
    let kept_vars = nvars.saturating_sub(1).min(MAX_DECODED_TRACES);

    // LTspice's compressed transient format can encode negative time stamps.
    let transient = RawAnalysisKind::classify(&meta.plot_name, meta.is_complex, total_points)
        == RawAnalysisKind::Transient;
    let mut x_values = Vec::with_capacity(total_points / step + 1);
    let mut reals: Vec<Vec<f64>> = vec![Vec::new(); kept_vars];
    let mut phases: Vec<Vec<f64>> = if meta.is_complex {
        vec![Vec::new(); kept_vars]
    } else {
        Vec::new()
    };

    for pt in (0..total_points).step_by(step) {
        if pt.is_multiple_of(512) {
            check_cancelled(cancelled, "LTspice binary decode cancelled")?;
        }
        let rec = &payload[pt * stride_bytes..(pt + 1) * stride_bytes];
        let x_val = f64::from_le_bytes(rec[0..8].try_into().unwrap_or([0; 8]));
        x_values.push(if transient { x_val.abs() } else { x_val });

        for v in 0..kept_vars {
            let var_idx = v + 1;
            if meta.is_complex {
                let off = var_idx * 16;
                let re = f64::from_le_bytes(rec[off..off + 8].try_into().unwrap_or([0; 8]));
                let im = f64::from_le_bytes(rec[off + 8..off + 16].try_into().unwrap_or([0; 8]));
                let mag = re.hypot(im).max(1e-18);
                reals[v].push(20.0 * mag.log10());
                phases[v].push(im.atan2(re).to_degrees());
            } else if meta.is_double {
                let off = var_idx * 8;
                let val = f64::from_le_bytes(rec[off..off + 8].try_into().unwrap_or([0; 8]));
                reals[v].push(val);
            } else {
                let off = 8 + v * 4;
                let val = f32::from_le_bytes(rec[off..off + 4].try_into().unwrap_or([0; 4]));
                reals[v].push(f64::from(val));
            }
        }
    }

    build_plot_from_vecs(meta, total_points, x_values, reals, phases)
}

fn decode_ltspice_ascii_plot(
    meta: &RawHeaderMeta,
    payload: &[u8],
    cancelled: &AtomicBool,
) -> Result<SpiceRawPlot, String> {
    let text = decode_spice_text(payload);
    let mut tokens = text.split_whitespace();
    let step = decimate_stride(meta.npoints);
    let kept_vars = (meta.variables.len() - 1).min(MAX_DECODED_TRACES);
    let mut x_values = Vec::new();
    let mut reals = vec![Vec::new(); kept_vars];
    let mut phases = vec![Vec::new(); if meta.is_complex { kept_vars } else { 0 }];

    for point in 0..meta.npoints {
        if point.is_multiple_of(512) {
            check_cancelled(cancelled, "SPICE ASCII raw decode cancelled")?;
        }
        if tokens.next().and_then(|token| token.parse::<usize>().ok()) != Some(point) {
            return Err("Invalid or missing SPICE .raw point index".into());
        }
        let (x, _) = parse_raw_scalar(tokens.next(), meta.is_complex)?;
        let keep = point.is_multiple_of(step);
        if keep {
            x_values.push(x);
        }
        for var in 0..meta.variables.len() - 1 {
            let (re, im) = parse_raw_scalar(tokens.next(), meta.is_complex)?;
            if keep && var < kept_vars {
                if meta.is_complex {
                    reals[var].push(20.0 * re.hypot(im).max(1e-18).log10());
                    phases[var].push(im.atan2(re).to_degrees());
                } else {
                    reals[var].push(re);
                }
            }
        }
    }
    if tokens.next().is_some() {
        return Err("Unexpected data after SPICE .raw values".into());
    }

    build_plot_from_vecs(meta, meta.npoints, x_values, reals, phases)
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

fn build_plot_from_vecs(
    meta: &RawHeaderMeta,
    total_points: usize,
    x_values: Vec<f64>,
    reals: Vec<Vec<f64>>,
    mut phases: Vec<Vec<f64>>,
) -> Result<SpiceRawPlot, String> {
    let mut traces = Vec::with_capacity(reals.len());
    for (v, values) in reals.into_iter().enumerate() {
        let (min, max) = min_max_finite(&values);
        let (name, unit_type) = meta
            .variables
            .get(v + 1)
            .cloned()
            .unwrap_or_else(|| (format!("v{}", v + 1), String::new()));
        let phase_deg = if meta.is_complex && !phases.is_empty() {
            Some(std::mem::take(&mut phases[v]))
        } else {
            None
        };
        traces.push(SpiceRawTrace {
            name,
            unit_type,
            values,
            phase_deg,
            min,
            max,
        });
    }
    let (x_name, x_unit) = meta
        .variables
        .first()
        .cloned()
        .unwrap_or_else(|| ("x".into(), String::new()));
    let kind = RawAnalysisKind::classify(&meta.plot_name, meta.is_complex, total_points);
    Ok(SpiceRawPlot {
        title: if meta.title.trim().is_empty() {
            "LTspice Simulation".into()
        } else {
            meta.title.clone()
        },
        date: meta.date.clone(),
        plot_name: meta.plot_name.clone(),
        kind,
        is_complex: meta.is_complex,
        total_points,
        total_vars: meta.variables.len(),
        x_name,
        x_unit,
        x_values,
        traces,
    }
    .include_first_operating_point_signal())
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
        .map(|t| t.values.first().copied().unwrap_or(0.0).abs())
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
    if plot.x_values.len() < 2 {
        return Ok(());
    }
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;
    let pad = 6.0_f32;
    let uw = (w - 2.0 * pad).max(4.0);
    let uh = (h - 2.0 * pad).max(4.0);

    let log_x = plot.kind == RawAnalysisKind::AcSweep && plot.x_values.iter().all(|&x| x > 0.0);
    let x_mapped: Vec<f64> = plot
        .x_values
        .iter()
        .map(|&x| if log_x { x.log10() } else { x })
        .collect();
    let (xmin, xmax) = min_max_finite(&x_mapped);
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
        let n = x_mapped.len().min(trace.values.len());
        if n < 2 {
            continue;
        }
        let [r, g, b] = TRACE_PALETTE[t_idx % TRACE_PALETTE.len()];
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, 235);
        paint.anti_alias = true;
        let mut pb = PathBuilder::new();
        for i in 0..n {
            let yv = trace.values[i];
            if !yv.is_finite() || !x_mapped[i].is_finite() {
                continue;
            }
            let px = pad + ((x_mapped[i] - xmin) / xspan) as f32 * uw;
            let py = pad + (1.0 - (yv - ymin) / yspan) as f32 * uh;
            if i == 0 {
                pb.move_to(px, py);
            } else {
                pb.line_to(px, py);
            }
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
        index: 0,
        log_x,
        log_y,
        traces,
    })
}

fn extract_quoted_or_csv_traces(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find('"') {
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
    if out.is_empty() {
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
    let panes = doc.panes.len().min(4);
    for (index, pane) in doc.panes.iter().take(panes).enumerate() {
        let height = h / panes as f32;
        let top = index as f32 * height;
        let mut paint = Paint::default();
        paint.set_color_rgba8(38, 46, 58, 255);
        if let Some(box_rect) = Rect::from_xywh(3.0, top + 2.0, w - 6.0, height - 4.0) {
            pixmap.fill_rect(box_rect, &paint, Transform::identity(), None);
        }
        for trace in 0..pane.traces.len().min(8) {
            let [r, g, b] = TRACE_PALETTE[trace];
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
        assert!(ac_plot.traces[0].phase_deg.is_some());
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
        assert!((lt_plot.traces[0].values[3] - 3.75).abs() < 1e-4);

        let fake_tiff = b"II*\0\x08\0\0\0\x01\0\x00\x01\x03\0\x01\0\0\0";
        assert!(!is_spice_raw_bytes(fake_tiff));
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
            .map(|t| (t.name.as_str(), t.values[0]))
            .collect();
        assert_eq!(values, [("V(in)", 5.0), ("V(out)", 3.3)]);
        assert!(plot.x_values.is_empty());
    }

    #[test]
    fn dc_sweep_preserves_negative_axis_values() {
        let header = "Title: Sweep\nPlotname: DC transfer characteristic\nFlags: real\nNo. Variables: 2\nNo. Points: 3\nVariables:\n0 v-sweep voltage\n1 V(out) voltage\n";
        let cancel = AtomicBool::new(false);
        let ascii = format!("{header}Values:\n0 -1 -2\n1 0 0\n2 1 2\n");
        let doc = SpiceRawDocument::parse(ascii.as_bytes(), &cancel).unwrap();
        assert_eq!(doc.primary().unwrap().x_values, [-1.0, 0.0, 1.0]);

        let mut binary = format!("{header}Binary:\n").into_bytes();
        for x in [-1.0_f64, 0.0, 1.0] {
            binary.extend_from_slice(&x.to_le_bytes());
            binary.extend_from_slice(&(2.0 * x as f32).to_le_bytes());
        }
        let doc = SpiceRawDocument::parse(&binary, &cancel).unwrap();
        assert_eq!(doc.primary().unwrap().x_values, [-1.0, 0.0, 1.0]);
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
        assert_eq!(doc.analysis, "Transient Analysis");
        assert_eq!(doc.panes.len(), 2);
        assert_eq!(doc.trace_count, 3);

        let hpgl = "IN;SP1;PU0,0;PD1000,0,1000,1000;SP0;";
        assert!(!looks_like_ltspice_plt(hpgl.as_bytes()));
    }
}
