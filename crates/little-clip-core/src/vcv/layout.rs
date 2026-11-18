//! Sugiyama layered layout, barycenter crossing reduction, port tabs, orthogonal routing,
//! compact strip projection, and vertical-budget dynamic unfolding.

use super::analyze::{
    AnalyzedCable, AnalyzedModule, Band, PatchAnalysis, PatchGroup, SignalType, build_patch_groups,
    graph_depths, is_modulation_input,
};
use super::catalog::{ModuleFunction, Role};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

pub const NODE_WIDTH: f64 = 196.0;
pub const NODE_HEIGHT: f64 = 52.0;
const MIN_GAP: f64 = 128.0;
const CHANNEL_PITCH: f64 = 12.0;
const GAP_PADDING: f64 = 32.0;
const ROW_SPACING: f64 = 16.0;
const TRACK_SPACING: f64 = 16.0;
const MIN_RUN_SPACING: f64 = 8.0;
const PORT_SPACING: f64 = 14.0;
pub const ARROW_LENGTH: f64 = 11.0;
const BAND_HEADER_HEIGHT: f64 = 28.0;
const BAND_PADDING: f64 = 12.0;
const MARGIN: f64 = 20.0;

pub const PORT_TAB_HEIGHT: f64 = 12.0;

/// How scopes and displays (`Band::Monitor`) are represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MonitorView {
    Hidden,
    #[default]
    Badge,
    Modules,
}

/// Visual specification for a miniature Eurorack module panel inside a node box.
#[derive(Clone, Debug)]
pub struct ModulePanelSpec {
    pub plugin: String,
    pub model: String,
    pub catalog_key: String,
    pub title: String,
    pub role: Role,
    pub function: ModuleFunction,
    pub hp_estimate: u8,
    pub knob_positions: Vec<f32>,
    pub input_count: usize,
    pub output_count: usize,
    pub count: usize,
}

/// Port tab attached to the left or right edge of a node box.
#[derive(Clone, Debug)]
pub struct PortTab {
    pub output: bool,
    pub y: f64,
    pub width: f64,
    pub text: String,
    pub signal: SignalType,
    pub tooltip: String,
}

/// A rendered box (either an unfolded module or a folded group).
#[derive(Clone, Debug)]
pub struct DiagramNode {
    pub key: String,
    pub module_id: Option<i64>,
    pub group: String,
    pub is_folded: bool,
    pub title: String,
    pub subtitle: String,
    pub role: Role,
    pub band: Band,
    pub is_insert: bool,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub watchers: Vec<String>,
    pub function: Option<ModuleFunction>,
    pub panels: Vec<ModulePanelSpec>,
    pub tabs: Vec<PortTab>,
}

impl DiagramNode {
    pub fn in_tabs_width(&self) -> f64 {
        self.tabs
            .iter()
            .filter(|t| !t.output)
            .map(|t| t.width)
            .fold(0.0, f64::max)
    }

    pub fn out_tabs_width(&self) -> f64 {
        self.tabs
            .iter()
            .filter(|t| t.output)
            .map(|t| t.width)
            .fold(0.0, f64::max)
    }
}

/// A cable (or merged bundle of same-signal cables) in the layout.
#[derive(Clone, Debug)]
pub struct DiagramEdge {
    pub key: String,
    pub from: String,
    pub to: String,
    pub cable_ids: Vec<i64>,
    pub from_title: String,
    pub from_port: String,
    pub to_title: String,
    pub to_port: String,
    pub signal: SignalType,
    pub intent: String,
    pub path: String,
    pub start_x: f64,
    pub start_y: f64,
    pub end_x: f64,
    pub end_y: f64,
    pub end_dir: f64,
    pub label_x: f64,
    pub label_y: f64,
    pub is_feedback: bool,
    pub is_monitor: bool,
}

impl DiagramEdge {
    pub fn label(&self) -> String {
        const MAX_CHARS: usize = 34;
        if self.intent.chars().count() <= MAX_CHARS {
            self.intent.clone()
        } else {
            let truncated: String = self.intent.chars().take(MAX_CHARS - 1).collect();
            format!("{}…", truncated.trim_end())
        }
    }

    pub fn is_single_cable(&self) -> bool {
        self.cable_ids.len() == 1
    }
}

/// Horizontal lane or vertical monitor column grouping boxes.
#[derive(Clone, Debug)]
pub struct DiagramBand {
    pub band: Band,
    pub group: Option<String>,
    pub title: String,
    pub x: Option<f64>,
    pub width: Option<f64>,
    pub y: f64,
    pub height: f64,
}

/// Complete diagram layout produced by `LayeredLayout::build`.
#[derive(Clone, Debug)]
pub struct DiagramLayout {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub bands: Vec<DiagramBand>,
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
    pub groups: Vec<PatchGroup>,
    pub is_strip: bool,
}

type EdgeKey = (String, String, SignalType);

/// Builds the full Sugiyama layered diagram layout, defaulting monitors to `MonitorView::Badge`.
pub fn build_diagram_layout(
    analysis: &PatchAnalysis,
    title: &str,
    unfolded: &BTreeSet<String>,
    monitors: MonitorView,
    port_tabs: bool,
) -> DiagramLayout {
    let folding = Folding::new(analysis, unfolded, monitors);
    let stacks = build_stacks(analysis, &folding);
    let merged = merge_cables(&folding, &stacks);
    let columns = assign_columns(analysis, &stacks, &merged);
    let lanes = assign_lanes(&folding, &stacks.boxes);
    let mut lane_of = build_lane_of(&lanes);
    let routing = plan_tracks(&merged, &stacks, &columns, &mut lane_of);
    let order = barycenter_order(
        initial_order(&stacks.boxes, &columns, &routing.track_column),
        &routing.segments,
        |e| lane_of[e],
        &routing.offsets,
        |e| routing.track_column.contains_key(e),
    );

    let mut trunk_of: HashMap<EdgeKey, String> = HashMap::new();
    for (k, group) in &merged {
        let mut parts: Vec<String> = group
            .iter()
            .map(|c| {
                format!(
                    "{}:{}:{}",
                    c.cable.from.module_id, c.cable.from.port_id, c.from_port
                )
            })
            .collect();
        parts.sort();
        parts.dedup();
        trunk_of.insert(k.clone(), format!("{}|{:?}|{}", k.0, k.2, parts.join(",")));
    }

    let height_of = box_heights(stacks.anchor_of.keys(), &merged, &trunk_of);
    let placement = place_rows(
        &lanes, &lane_of, &order, &routing, &stacks, &columns, &height_of, &merged,
    );
    let ports = place_ports(&merged, &trunk_of, &routing.plans, &placement);
    let mut turns = find_turns(&routing.plans, &ports.levels);
    turns.extend(side_loop_turns(
        &merged,
        &routing.plans,
        &ports,
        &placement.positions,
    ));
    let tabs = Tabs::new(
        &merged,
        &routing.plans,
        &ports,
        port_tabs,
        &placement.positions,
    );
    let gaps = Gaps::new(&turns, &tabs, columns.drawn, |turn| {
        if turn.part == 0 && !turn.feedback {
            trunk_of.get(&turn.key).map(String::as_str)
        } else {
            None
        }
    });

    let mut bands = placement.bands;
    if !stacks.watchers.is_empty()
        && let Some(watchers_group) = folding.groups.iter().find(|g| g.band == Band::Monitor)
    {
        let left = gaps.column_x[columns.count] - tabs.in_width(columns.count) - 12.0;
        bands.push(DiagramBand {
            band: Band::Monitor,
            group: folding
                .has_own_lane(watchers_group)
                .then(|| watchers_group.key.clone()),
            title: Band::Monitor.box_title().to_string(),
            x: Some(left),
            width: Some(gaps.column_x[columns.count] + NODE_WIDTH + 16.0 - left),
            y: MARGIN,
            height: placement.watchers_bottom - MARGIN,
        });
    }

    let nodes = make_nodes(
        analysis,
        &folding,
        &placement.positions,
        &placement.height_of,
        &gaps.column_x,
        &tabs,
        monitors,
    );
    let node_by_key: HashMap<String, &DiagramNode> =
        nodes.iter().map(|n| (n.key.clone(), n)).collect();
    let mut runs: HashMap<String, Vec<LabelRun>> = HashMap::new();
    let edges = draw_edges(
        analysis,
        &merged,
        &node_by_key,
        &routing.plans,
        &ports.levels,
        &ports.out_y,
        &ports.in_y,
        &gaps.channels,
        &mut runs,
        |k| tabs.out_tab_width(k),
        |k| tabs.in_tab_width(k),
    );
    let edges = place_edge_labels(edges, &runs, &nodes);
    let width = gaps.right(columns.drawn.saturating_sub(1)) + MARGIN;
    let height = placement.y - 8.0 + MARGIN;

    DiagramLayout {
        title: title.to_string(),
        width,
        height,
        bands,
        nodes,
        edges,
        groups: folding.groups,
        is_strip: false,
    }
}

/// Builds a compact single-row strip overview for inline display inside file manager list rows.
///
/// The strip preserves the folded overview and left-to-right signal flow while collapsing bands
/// vertically into a single horizontal ribbon with orthogonal signal connections, miniature panel
/// previews, and `👁` monitor badges.
pub fn build_strip_layout(analysis: &PatchAnalysis, title: &str) -> DiagramLayout {
    let groups: Vec<PatchGroup> = build_patch_groups(analysis)
        .into_iter()
        .filter(|g| g.band != Band::Monitor)
        .collect();
    if groups.is_empty() {
        return DiagramLayout {
            title: title.to_string(),
            width: 320.0,
            height: 68.0,
            bands: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            groups,
            is_strip: true,
        };
    }

    let unfolded = BTreeSet::new();
    let folding = Folding::new(analysis, &unfolded, MonitorView::Badge);
    let stacks = build_stacks(analysis, &folding);
    let merged = merge_cables(&folding, &stacks);
    let columns = assign_columns(analysis, &stacks, &merged);

    // Order boxes strictly left-to-right by topological column, then band priority.
    let mut ordered_keys: Vec<String> = stacks.boxes.clone();
    ordered_keys.sort_by_key(|k| {
        let col = columns.of.get(k).copied().unwrap_or(0);
        let band = folding
            .group_for_box_key(k)
            .map_or(Band::Modulation, |g| g.band);
        (col, band)
    });

    const STRIP_HEIGHT: f64 = 66.0;
    const BOX_Y: f64 = 6.0;
    const BOX_H: f64 = 52.0;
    const BOX_W: f64 = 166.0;
    const BOX_GAP: f64 = 24.0;

    let mut nodes = Vec::with_capacity(ordered_keys.len());
    let mut x = 6.0;
    let watchers = Watched::new(analysis, &folding);

    for (col_idx, box_key) in ordered_keys.iter().enumerate() {
        let node = build_single_node_for_strip(
            analysis, &folding, box_key, x, BOX_Y, BOX_W, BOX_H, &watchers,
        );
        let _ = col_idx;
        nodes.push(node);
        x += BOX_W + BOX_GAP;
    }

    // Build compact strip edges between the ordered boxes (top bus channels for arcs).
    let node_by_key: HashMap<String, (usize, &DiagramNode)> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.key.clone(), (i, n)))
        .collect();

    let mut edges = Vec::new();
    for (edge_idx, (key, group_cables)) in merged.iter().enumerate() {
        let Some(&(from_idx, from_node)) = node_by_key.get(&key.0) else {
            continue;
        };
        let Some(&(to_idx, to_node)) = node_by_key.get(&key.1) else {
            continue;
        };
        let y_slot = 0.25 + 0.5 * ((edge_idx % 4) as f64 / 3.0);
        let start_y = from_node.y + from_node.height * y_slot;
        let end_y = to_node.y + to_node.height * y_slot;

        let (start_x, end_x, path, feedback) = if to_idx == from_idx + 1 {
            // Direct adjacent neighbor in the single strip: short straight arrow.
            let sx = from_node.x + from_node.width;
            let ex = to_node.x - 6.0;
            let mid_y = (start_y + end_y) * 0.5;
            (sx, ex, format!("M {sx:.1} {mid_y:.1} H {ex:.1}"), false)
        } else if to_idx > from_idx + 1 {
            // Forward skip over intermediate boxes: route subtly along bottom rail.
            let sx = from_node.x + from_node.width - 4.0;
            let ex = to_node.x - 6.0;
            let rail_y = BOX_Y + BOX_H + 4.0 + ((edge_idx % 3) as f64) * 2.5;
            let x_out = from_node.x + from_node.width + 8.0;
            let x_in = to_node.x - 10.0;
            (
                sx,
                ex,
                format!(
                    "M {sx:.1} {start_y:.1} H {x_out:.1} V {rail_y:.1} H {x_in:.1} V {end_y:.1} H {ex:.1}"
                ),
                false,
            )
        } else {
            // Feedback / reverse connection: dashed arc above the boxes.
            let sx = from_node.x + from_node.width * 0.72;
            let sy = from_node.y;
            let ex = to_node.x + to_node.width * 0.28;
            let ey = to_node.y + 4.0;
            let top_y = 2.0 + ((edge_idx % 2) as f64) * 2.0;
            (
                sx,
                ex,
                format!("M {sx:.1} {sy:.1} V {top_y:.1} H {ex:.1} V {ey:.1}"),
                true,
            )
        };

        let first = &group_cables[0];
        edges.push(DiagramEdge {
            key: format!("{}>{}:{:?}", key.0, key.1, key.2),
            from: key.0.clone(),
            to: key.1.clone(),
            cable_ids: group_cables.iter().map(|c| c.cable.id).collect(),
            from_title: from_node.title.clone(),
            from_port: first.from_port.clone(),
            to_title: to_node.title.clone(),
            to_port: first.to_port.clone(),
            signal: key.2,
            intent: String::new(),
            path,
            start_x,
            start_y,
            end_x,
            end_y,
            end_dir: if feedback { 0.0 } else { 1.0 },
            label_x: 0.0,
            label_y: 0.0,
            is_feedback: feedback,
            is_monitor: false,
        });
    }

    let total_width = (x - BOX_GAP + 6.0).max(200.0);
    DiagramLayout {
        title: title.to_string(),
        width: total_width,
        height: STRIP_HEIGHT,
        bands: Vec::new(),
        nodes,
        edges,
        groups: folding.groups,
        is_strip: true,
    }
}

/// Dynamically unfolds `PatchGroup` rows on hover to expand as much as possible without
/// overflowing the available vertical space (`max_height_px`).
pub fn build_dynamic_hover_layout(
    analysis: &PatchAnalysis,
    title: &str,
    max_width_px: f64,
    max_height_px: f64,
) -> DiagramLayout {
    let mut unfolded = BTreeSet::new();
    let mut best = build_diagram_layout(analysis, title, &unfolded, MonitorView::Badge, true);

    // Prioritize unfolding groups with multiple modules in musical signal order:
    // voices first, then bus, sequencing/time, pitch, modulation, external.
    let mut candidates: Vec<(u8, usize, String)> = best
        .groups
        .iter()
        .filter(|g| g.members.len() > 1 && g.band != Band::Monitor)
        .map(|g| {
            let priority = match g.band {
                Band::Voice => 0,
                Band::Bus => 1,
                Band::Time => 2,
                Band::Pitch => 3,
                Band::Modulation => 4,
                Band::External => 5,
                Band::Monitor => 6,
            };
            (priority, g.members.len(), g.key.clone())
        })
        .collect();
    candidates.sort();

    for (_, _, group_key) in candidates {
        let mut trial = unfolded.clone();
        trial.insert(group_key);
        let candidate = build_diagram_layout(analysis, title, &trial, MonitorView::Badge, true);
        let width_scale = (max_width_px / candidate.width.max(1.0)).min(1.0);
        let projected_height = candidate.height * width_scale;
        // Keep a readability floor (>= 0.48x) while expanding vertically up to max_height_px.
        let min_scale = (max_height_px / candidate.height.max(1.0)).min(width_scale);
        if projected_height <= max_height_px && min_scale >= 0.48 {
            unfolded = trial;
            best = candidate;
        }
    }

    best
}

// =========================================================================
// Internal layout machinery (LayeredLayout port)
// =========================================================================

pub fn module_key(id: i64) -> String {
    format!("m{id}")
}

pub fn group_key(key: &str) -> String {
    format!("g:{key}")
}

fn module_id_of(key: &str) -> Option<i64> {
    key.strip_prefix('m').and_then(|s| s.parse::<i64>().ok())
}

struct Folding {
    unfolded: BTreeSet<String>,
    groups: Vec<PatchGroup>,
    visible: HashSet<i64>,
    cables: Vec<AnalyzedCable>,
    group_of: HashMap<i64, usize>,
    box_of: HashMap<i64, String>,
}

impl Folding {
    fn new(analysis: &PatchAnalysis, unfolded: &BTreeSet<String>, monitors: MonitorView) -> Self {
        let groups: Vec<PatchGroup> = build_patch_groups(analysis)
            .into_iter()
            .filter(|g| monitors == MonitorView::Modules || g.band != Band::Monitor)
            .collect();
        let visible: HashSet<i64> = groups
            .iter()
            .flat_map(|g| g.members.iter().copied())
            .collect();
        let cables: Vec<AnalyzedCable> = analysis
            .cables
            .iter()
            .filter(|c| {
                visible.contains(&c.cable.from.module_id) && visible.contains(&c.cable.to.module_id)
            })
            .cloned()
            .collect();
        let mut group_of = HashMap::new();
        for (gi, g) in groups.iter().enumerate() {
            for &id in &g.members {
                group_of.insert(id, gi);
            }
        }
        let mut box_of = HashMap::new();
        for (&id, &gi) in &group_of {
            let g = &groups[gi];
            let key = if Self::shows_modules_inner(unfolded, g) {
                module_key(id)
            } else {
                group_key(&g.key)
            };
            box_of.insert(id, key);
        }
        Self {
            unfolded: unfolded.clone(),
            groups,
            visible,
            cables,
            group_of,
            box_of,
        }
    }

    fn shows_modules_inner(unfolded: &BTreeSet<String>, group: &PatchGroup) -> bool {
        unfolded.contains(&group.key) || group.members.len() == 1
    }

    fn is_unfolded(&self, group: &PatchGroup) -> bool {
        self.unfolded.contains(&group.key)
    }

    fn shows_modules(&self, group: &PatchGroup) -> bool {
        Self::shows_modules_inner(&self.unfolded, group)
    }

    fn has_own_lane(&self, group: &PatchGroup) -> bool {
        self.is_unfolded(group) && group.members.len() > 1
    }

    fn box_of(&self, id: i64) -> &str {
        &self.box_of[&id]
    }

    fn single_box(&self, group: &PatchGroup) -> String {
        if self.shows_modules(group) {
            module_key(group.members[0])
        } else {
            group_key(&group.key)
        }
    }

    fn group_for_box_key(&self, box_key: &str) -> Option<&PatchGroup> {
        if let Some(gk) = box_key.strip_prefix("g:") {
            self.groups.iter().find(|g| g.key == gk)
        } else if let Some(mid) = module_id_of(box_key) {
            self.group_of.get(&mid).map(|&gi| &self.groups[gi])
        } else {
            None
        }
    }
}

struct Stacks {
    boxes: Vec<String>,
    watchers: Vec<String>,
    watching: HashSet<String>,
    stack_of: HashMap<String, Vec<String>>,
    anchor_of: HashMap<String, String>,
}

fn build_stacks(analysis: &PatchAnalysis, folding: &Folding) -> Stacks {
    let mut boxes = Vec::new();
    let mut watchers = Vec::new();
    for g in &folding.groups {
        if g.band == Band::Monitor {
            if folding.shows_modules(g) {
                watchers.extend(g.members.iter().copied().map(module_key));
            } else {
                watchers.push(group_key(&g.key));
            }
        } else if folding.shows_modules(g) {
            for &id in &g.members {
                let m = analysis.module(id).expect("member in analysis");
                let stacked_under_host = m.insert_of.is_some_and(|host| g.members.contains(&host));
                if !stacked_under_host {
                    boxes.push(module_key(id));
                }
            }
        } else {
            boxes.push(group_key(&g.key));
        }
    }

    let placed: HashSet<String> = boxes.iter().cloned().collect();
    let mut inserts_of: HashMap<i64, Vec<&AnalyzedModule>> = HashMap::new();
    for m in &analysis.modules {
        if let Some(host) = m.insert_of {
            inserts_of.entry(host).or_default().push(m);
        }
    }
    for list in inserts_of.values_mut() {
        list.sort_by_key(|m| (m.module.row, m.module.column, m.module.id));
    }

    let mut stack_of = HashMap::new();
    let mut anchor_of = HashMap::new();
    for box_key in &boxes {
        let mut stack = vec![box_key.clone()];
        if let Some(id) = module_id_of(box_key)
            && let Some(inserts) = inserts_of.get(&id)
        {
            for ins in inserts {
                let k = module_key(ins.module.id);
                if !anchor_of.contains_key(&k) && !placed.contains(&k) {
                    stack.push(k);
                }
            }
        }
        for k in &stack {
            anchor_of
                .entry(k.clone())
                .or_insert_with(|| box_key.clone());
        }
        stack_of.insert(box_key.clone(), stack);
    }
    for w in &watchers {
        stack_of.insert(w.clone(), vec![w.clone()]);
        anchor_of.insert(w.clone(), w.clone());
    }

    let watching: HashSet<String> = watchers.iter().cloned().collect();
    Stacks {
        boxes,
        watchers,
        watching,
        stack_of,
        anchor_of,
    }
}

fn merge_cables(folding: &Folding, stacks: &Stacks) -> Vec<(EdgeKey, Vec<AnalyzedCable>)> {
    let mut map: BTreeMap<EdgeKey, Vec<AnalyzedCable>> = BTreeMap::new();
    for c in &folding.cables {
        let from_box = folding.box_of(c.cable.from.module_id).to_string();
        let to_box = folding.box_of(c.cable.to.module_id).to_string();
        let is_self_patch = c.cable.from.module_id == c.cable.to.module_id
            && from_box == module_key(c.cable.from.module_id);
        if (from_box != to_box || is_self_patch)
            && !stacks.watching.contains(&from_box)
            && stacks.anchor_of.contains_key(&from_box)
            && stacks.anchor_of.contains_key(&to_box)
        {
            map.entry((from_box, to_box, c.signal))
                .or_default()
                .push(c.clone());
        }
    }
    map.into_iter().collect()
}

struct Columns {
    of: HashMap<String, usize>,
    count: usize,
    drawn: usize,
}

fn assign_columns(
    analysis: &PatchAnalysis,
    stacks: &Stacks,
    merged: &[(EdgeKey, Vec<AnalyzedCable>)],
) -> Columns {
    let mut by_link: BTreeMap<(String, String), Vec<&AnalyzedCable>> = BTreeMap::new();
    for (k, cables) in merged {
        let a_from = stacks.anchor_of[&k.0].clone();
        let a_to = stacks.anchor_of[&k.1].clone();
        if a_from != a_to && !stacks.watching.contains(&a_to) {
            let entry = by_link.entry((a_from, a_to)).or_default();
            entry.extend(cables.iter());
        }
    }

    let links: Vec<(String, String)> = by_link.keys().cloned().collect();
    let weak: HashSet<(String, String)> = by_link
        .iter()
        .filter(|(_, cs)| cs.iter().all(|c| is_modulation_input(&c.to_port)))
        .map(|(k, _)| k.clone())
        .collect();

    let box_indices: Vec<i64> = (0..stacks.boxes.len() as i64).collect();
    let idx_of: HashMap<&str, i64> = stacks
        .boxes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.as_str(), i as i64))
        .collect();
    let num_links: Vec<(i64, i64)> = links
        .iter()
        .map(|(f, t)| (idx_of[f.as_str()], idx_of[t.as_str()]))
        .collect();
    let num_weak: HashSet<(i64, i64)> = weak
        .iter()
        .map(|(f, t)| (idx_of[f.as_str()], idx_of[t.as_str()]))
        .collect();
    let depths = graph_depths(&box_indices, &num_links, &num_weak);
    let mut column: HashMap<String, usize> = stacks
        .boxes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.clone(), depths[&(i as i64)]))
        .collect();
    let count = column.values().copied().max().unwrap_or(0) + 1;

    let sending: HashSet<&str> = links.iter().map(|(f, _)| f.as_str()).collect();
    for b in &stacks.boxes {
        if let Some(id) = module_id_of(b)
            && !sending.contains(b.as_str())
            && links.iter().any(|(_, t)| t == b)
            && analysis.module(id).is_some_and(|m| m.role == Role::Io)
        {
            column.insert(b.clone(), count - 1);
        }
    }

    Columns {
        of: column,
        count,
        drawn: count + usize::from(!stacks.watchers.is_empty()),
    }
}

struct Lane {
    band: Band,
    group: Option<String>,
    title: String,
    boxes: Vec<String>,
}

fn assign_lanes(folding: &Folding, boxes: &[String]) -> Vec<Lane> {
    let mut lanes = Vec::new();
    for band in [
        Band::External,
        Band::Time,
        Band::Pitch,
        Band::Voice,
        Band::Bus,
        Band::Modulation,
    ] {
        let mut folded = Vec::new();
        let close_folded = |lanes: &mut Vec<Lane>, folded: &mut Vec<String>| {
            if !folded.is_empty() {
                lanes.push(Lane {
                    band,
                    group: None,
                    title: band.lane_title(false).to_string(),
                    boxes: std::mem::take(folded),
                });
            }
        };

        for group in folding.groups.iter().filter(|g| g.band == band) {
            if folding.has_own_lane(group) {
                close_folded(&mut lanes, &mut folded);
                let members: HashSet<i64> = group.members.iter().copied().collect();
                let lane_boxes: Vec<String> = boxes
                    .iter()
                    .filter(|b| module_id_of(b).is_some_and(|id| members.contains(&id)))
                    .cloned()
                    .collect();
                lanes.push(Lane {
                    band,
                    group: Some(group.key.clone()),
                    title: group.title.clone(),
                    boxes: lane_boxes,
                });
            } else if band == Band::Voice {
                close_folded(&mut lanes, &mut folded);
                let title = match group.title.find(": ") {
                    Some(idx) if idx > 0 => group.title[..idx].to_string(),
                    _ => group.title.clone(),
                };
                lanes.push(Lane {
                    band,
                    group: None,
                    title,
                    boxes: vec![folding.single_box(group)],
                });
            } else {
                folded.push(folding.single_box(group));
            }
        }
        close_folded(&mut lanes, &mut folded);
    }
    lanes
}

fn build_lane_of(lanes: &[Lane]) -> HashMap<String, usize> {
    let mut lane_of = HashMap::new();
    for (lane_idx, lane) in lanes.iter().enumerate() {
        for b in &lane.boxes {
            lane_of.insert(b.clone(), lane_idx);
        }
    }
    lane_of
}

struct Plan {
    tracks: Vec<String>,
    gaps: Vec<i32>,
    feedback: bool,
}

struct Turn {
    gap: i32,
    key: EdgeKey,
    part: usize,
    y_from: f64,
    y_to: f64,
    feedback: bool,
}

struct Routing {
    plans: HashMap<EdgeKey, Plan>,
    track_column: HashMap<String, usize>,
    segments: Vec<(String, String)>,
    feedback_tracks: HashSet<String>,
    predecessor_of: HashMap<String, String>,
    offsets: Vec<(f64, f64)>,
}

fn plan_tracks(
    merged: &[(EdgeKey, Vec<AnalyzedCable>)],
    stacks: &Stacks,
    columns: &Columns,
    lane_of: &mut HashMap<String, usize>,
) -> Routing {
    let mut plans = HashMap::new();
    let mut track_column = HashMap::new();
    let mut segments = Vec::new();
    let mut offsets = Vec::new();

    let in_stack = |b: &str| -> f64 {
        let anchor = &stacks.anchor_of[b];
        let stack = &stacks.stack_of[anchor];
        let idx = stack.iter().position(|s| s == b).unwrap_or(0);
        idx as f64 / stack.len().max(1) as f64
    };

    for (key, _) in merged {
        let source = &stacks.anchor_of[&key.0];
        let target = &stacks.anchor_of[&key.1];
        if source == target && key.0 != key.1 {
            continue;
        }
        let first = columns.of[source];
        let last = if stacks.watching.contains(target) {
            columns.count
        } else {
            columns.of[target]
        };
        let feedback = last <= first;
        let track_cols: Vec<usize> = if feedback {
            (last..=first).rev().collect()
        } else if last > first + 1 {
            ((first + 1)..last).collect()
        } else {
            Vec::new()
        };
        let gaps: Vec<i32> = if feedback {
            ((last as i32 - 1)..=(first as i32)).rev().collect()
        } else {
            (first as i32..last as i32).collect()
        };

        let mut tracks = Vec::with_capacity(track_cols.len());
        let plan_id = plans.len();
        for &col in &track_cols {
            let track = format!("t{plan_id}:{col}");
            tracks.push(track.clone());
            track_column.insert(track.clone(), col);
            let src_lane = lane_of[source];
            lane_of.insert(track, src_lane);
        }
        plans.insert(
            key.clone(),
            Plan {
                tracks: tracks.clone(),
                gaps,
                feedback,
            },
        );
        if feedback {
            continue;
        }

        let mut chain = vec![source.clone()];
        chain.extend(tracks);
        if !stacks.watching.contains(target) {
            chain.push(target.clone());
        }
        for i in 0..chain.len().saturating_sub(1) {
            let off_u = if i == 0 { in_stack(&key.0) } else { 0.0 };
            let off_v = if i + 2 == chain.len() && chain.last() == Some(target) {
                in_stack(&key.1)
            } else {
                0.0
            };
            segments.push((chain[i].clone(), chain[i + 1].clone()));
            offsets.push((off_u, off_v));
        }
    }

    let feedback_tracks: HashSet<String> = plans
        .values()
        .filter(|p| p.feedback)
        .flat_map(|p| p.tracks.iter().cloned())
        .collect();
    let predecessor_of: HashMap<String, String> = segments
        .iter()
        .filter(|(_, v)| track_column.contains_key(v))
        .map(|(u, v)| (v.clone(), u.clone()))
        .collect();

    Routing {
        plans,
        track_column,
        segments,
        feedback_tracks,
        predecessor_of,
        offsets,
    }
}

fn initial_order(
    boxes: &[String],
    columns: &Columns,
    track_column: &HashMap<String, usize>,
) -> Vec<Vec<String>> {
    let mut cols = vec![Vec::new(); columns.count];
    for b in boxes {
        if let Some(&c) = columns.of.get(b)
            && c < cols.len()
        {
            cols[c].push(b.clone());
        }
    }
    let mut track_entries: Vec<(&String, usize)> =
        track_column.iter().map(|(k, &c)| (k, c)).collect();
    track_entries.sort_by(|a, b| a.0.cmp(b.0));
    for (t, c) in track_entries {
        if c < cols.len() {
            cols[c].push(t.clone());
        }
    }
    cols
}

// =========================================================================
// Barycenter crossing reduction
// =========================================================================

fn barycenter_order(
    columns: Vec<Vec<String>>,
    segments: &[(String, String)],
    lane_of: impl Fn(&str) -> usize,
    offsets: &[(f64, f64)],
    is_track: impl Fn(&str) -> bool,
) -> Vec<Vec<String>> {
    struct End {
        element: String,
        offset: f64,
        direct: bool,
    }
    let mut left: HashMap<String, Vec<End>> = HashMap::new();
    let mut right: HashMap<String, Vec<End>> = HashMap::new();
    let is_direct = |i: usize| !is_track(&segments[i].0) && !is_track(&segments[i].1);

    for (i, (u, v)) in segments.iter().enumerate() {
        let dir = is_direct(i);
        let (off_u, off_v) = offsets[i];
        left.entry(v.clone()).or_default().push(End {
            element: u.clone(),
            offset: off_u,
            direct: dir,
        });
        right.entry(u.clone()).or_default().push(End {
            element: v.clone(),
            offset: off_v,
            direct: dir,
        });
    }

    let positions_of = |cols: &[Vec<String>]| -> HashMap<String, usize> {
        let mut at = HashMap::new();
        for c in cols {
            for (i, e) in c.iter().enumerate() {
                at.insert(e.clone(), i);
            }
        }
        at
    };

    let pair_cost = |d1: bool, d2: bool| 1 + 3 * (usize::from(d1) + usize::from(d2));
    let cost_of = |cols: &[Vec<String>]| -> usize {
        let at = positions_of(cols);
        let mut col_of = HashMap::new();
        for (i, c) in cols.iter().enumerate() {
            for e in c {
                col_of.insert(e.as_str(), i);
            }
        }
        let mut layers: BTreeMap<usize, Vec<(f64, f64, bool)>> = BTreeMap::new();
        for (i, (u, v)) in segments.iter().enumerate() {
            let l = col_of[u.as_str()];
            layers.entry(l).or_default().push((
                at[u] as f64 + offsets[i].0,
                at[v] as f64 + offsets[i].1,
                is_direct(i),
            ));
        }
        let mut cost = 0;
        for list in layers.into_values() {
            for i in 0..list.len() {
                for j in (i + 1)..list.len() {
                    if (list[i].0 - list[j].0) * (list[i].1 - list[j].1) < 0.0 {
                        cost += pair_cost(list[i].2, list[j].2);
                    }
                }
            }
        }
        cost
    };

    let mut current: Vec<Vec<String>> = columns
        .into_iter()
        .map(|mut c| {
            c.sort_by_key(|e| lane_of(e));
            c
        })
        .collect();
    let mut best = current.clone();
    let mut lowest = cost_of(&best);

    for sweep in 0..8 {
        if lowest == 0 {
            break;
        }
        let forward = sweep % 2 == 0;
        let neighbours = if forward { &left } else { &right };
        let col_indices: Vec<usize> = if forward {
            (1..current.len()).collect()
        } else {
            (0..current.len().saturating_sub(1)).rev().collect()
        };
        let mut at = positions_of(&current);
        for col in col_indices {
            current[col].sort_by(|a, b| {
                let la = lane_of(a);
                let lb = lane_of(b);
                if la != lb {
                    return la.cmp(&lb);
                }
                let ba = neighbours.get(a).map_or(at[a] as f64, |ns| {
                    ns.iter()
                        .map(|n| at[&n.element] as f64 + n.offset)
                        .sum::<f64>()
                        / ns.len() as f64
                });
                let bb = neighbours.get(b).map_or(at[b] as f64, |ns| {
                    ns.iter()
                        .map(|n| at[&n.element] as f64 + n.offset)
                        .sum::<f64>()
                        / ns.len() as f64
                });
                ba.partial_cmp(&bb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| at[a].cmp(&at[b]))
            });
            for (i, e) in current[col].iter().enumerate() {
                at.insert(e.clone(), i);
            }
        }
        let c = cost_of(&current);
        if c < lowest {
            lowest = c;
            best = current.clone();
        }
    }

    // Transpose refinement
    let mut at = positions_of(&best);
    let crossed = |upper: &str,
                   lower: &str,
                   side: &HashMap<String, Vec<End>>,
                   at: &HashMap<String, usize>|
     -> usize {
        let (Some(ups), Some(downs)) = (side.get(upper), side.get(lower)) else {
            return 0;
        };
        let mut total = 0;
        for u in ups {
            for d in downs {
                if at[&u.element] as f64 + u.offset > at[&d.element] as f64 + d.offset {
                    total += pair_cost(u.direct, d.direct);
                }
            }
        }
        total
    };
    let mut improved = true;
    while improved {
        improved = false;
        for col in &mut best {
            for i in 0..col.len().saturating_sub(1) {
                let upper = col[i].clone();
                let lower = col[i + 1].clone();
                if lane_of(&upper) != lane_of(&lower) {
                    continue;
                }
                let now =
                    crossed(&upper, &lower, &left, &at) + crossed(&upper, &lower, &right, &at);
                let swapped =
                    crossed(&lower, &upper, &left, &at) + crossed(&lower, &upper, &right, &at);
                if swapped < now {
                    col.swap(i, i + 1);
                    at.insert(lower, i);
                    at.insert(upper, i + 1);
                    improved = true;
                }
            }
        }
    }
    best
}

// =========================================================================
// Box sizing, Placement, Ports, Turns, Tabs, Gaps, Edges
// =========================================================================

fn box_heights<'a>(
    keys: impl Iterator<Item = &'a String>,
    merged: &[(EdgeKey, Vec<AnalyzedCable>)],
    trunk_of: &HashMap<EdgeKey, String>,
) -> HashMap<String, f64> {
    let mut leaving: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut arriving: HashMap<&str, usize> = HashMap::new();
    for (k, _) in merged {
        leaving
            .entry(k.0.as_str())
            .or_default()
            .insert(trunk_of[k].as_str());
        *arriving.entry(k.1.as_str()).or_default() += 1;
    }
    keys.map(|k| {
        let out_c = leaving.get(k.as_str()).map_or(0, HashSet::len);
        let in_c = arriving.get(k.as_str()).copied().unwrap_or(0);
        let h = NODE_HEIGHT.max((out_c.max(in_c) + 1) as f64 * PORT_SPACING);
        (k.clone(), h)
    })
    .collect()
}

struct Placement {
    bands: Vec<DiagramBand>,
    positions: HashMap<String, (usize, f64)>,
    track_y: HashMap<String, f64>,
    height_of: HashMap<String, f64>,
    y: f64,
    watchers_bottom: f64,
}

impl Placement {
    fn center(&self, key: &str) -> f64 {
        self.positions[key].1 + (self.height_of[key] / 2.0)
    }
}

#[allow(clippy::too_many_arguments)]
fn place_rows(
    lanes: &[Lane],
    lane_of: &HashMap<String, usize>,
    order: &[Vec<String>],
    routing: &Routing,
    stacks: &Stacks,
    columns: &Columns,
    height_of: &HashMap<String, f64>,
    merged: &[(EdgeKey, Vec<AnalyzedCable>)],
) -> Placement {
    let mut bands = Vec::new();
    let mut positions: HashMap<String, (usize, f64)> = HashMap::new();
    let mut track_y: HashMap<String, f64> = HashMap::new();

    let mut self_tracks: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut feedback_plans: HashMap<usize, Vec<&Plan>> = HashMap::new();
    for (k, plan) in &routing.plans {
        if k.0 == k.1 {
            if let Some(first) = plan.tracks.first() {
                self_tracks.entry(k.0.as_str()).or_default().push(first);
            }
        } else if plan.feedback && !plan.tracks.is_empty() {
            feedback_plans
                .entry(lane_of[&plan.tracks[0]])
                .or_default()
                .push(plan);
        }
    }

    let mut y = MARGIN;
    for (lane_idx, lane) in lanes.iter().enumerate() {
        let top = y + BAND_HEADER_HEIGHT;
        let mut bottom = top + NODE_HEIGHT;
        let mut free = vec![0.0; columns.count];
        for (col, col_elements) in order.iter().enumerate() {
            let here: Vec<&String> = col_elements
                .iter()
                .filter(|e| !routing.feedback_tracks.contains(*e) && lane_of[*e] == lane_idx)
                .collect();
            let last_box = here
                .iter()
                .rposition(|e| !routing.track_column.contains_key(*e));
            let mut cursor = top;
            for (i, &element) in here.iter().enumerate() {
                if routing.track_column.contains_key(element) {
                    let mut line = cursor;
                    if last_box.is_some_and(|lb| i > lb)
                        && let Some(before) = routing.predecessor_of.get(element)
                    {
                        let prev = track_y
                            .get(before)
                            .copied()
                            .unwrap_or_else(|| positions[before].1 + height_of[before] / 2.0);
                        line = line.max(prev);
                    }
                    track_y.insert(element.clone(), line);
                    cursor = line + TRACK_SPACING;
                    bottom = bottom.max(line + 4.0);
                    continue;
                }
                for key in &stacks.stack_of[element] {
                    positions.insert(key.clone(), (col, cursor));
                    bottom = bottom.max(cursor + height_of[key]);
                    cursor += height_of[key] + ROW_SPACING;
                    if let Some(st) = self_tracks.get(key.as_str()) {
                        for &track in st {
                            let ty = cursor - ROW_SPACING / 2.0;
                            track_y.insert(track.to_string(), ty);
                            bottom = bottom.max(ty + 4.0);
                            cursor += MIN_RUN_SPACING;
                        }
                    }
                }
            }
            if col < free.len() {
                free[col] = cursor;
            }
        }

        if let Some(fplans) = feedback_plans.get(&lane_idx) {
            for plan in fplans {
                let spanned: Vec<usize> = plan
                    .tracks
                    .iter()
                    .map(|t| routing.track_column[t])
                    .collect();
                let line = spanned
                    .iter()
                    .filter_map(|&c| free.get(c).copied())
                    .fold(0.0, f64::max);
                for track in &plan.tracks {
                    track_y.insert(track.clone(), line);
                }
                for &col in &spanned {
                    if col < free.len() {
                        free[col] = line + TRACK_SPACING;
                    }
                }
                bottom = bottom.max(line + 4.0);
            }
        }

        let height = bottom - y + BAND_PADDING;
        bands.push(DiagramBand {
            band: lane.band,
            group: lane.group.clone(),
            title: lane.title.clone(),
            x: None,
            width: None,
            y,
            height,
        });
        y += height + 8.0;
    }

    if stacks.watchers.is_empty() {
        return Placement {
            bands,
            positions,
            track_y,
            height_of: height_of.clone(),
            y,
            watchers_bottom: 0.0,
        };
    }

    let mut watched_from: HashMap<&str, Vec<&str>> = HashMap::new();
    for (k, _) in merged {
        watched_from
            .entry(k.1.as_str())
            .or_default()
            .push(k.0.as_str());
    }
    let center_of = |key: &str, positions: &HashMap<String, (usize, f64)>| -> f64 {
        positions[key].1 + height_of[key] / 2.0
    };
    let wanted = |w: &str, positions: &HashMap<String, (usize, f64)>| -> f64 {
        let Some(froms) = watched_from.get(w) else {
            return MARGIN;
        };
        if froms.is_empty() {
            MARGIN
        } else {
            froms.iter().map(|f| center_of(f, positions)).sum::<f64>() / froms.len() as f64
        }
    };
    let arriving = |w: &str,
                    positions: &HashMap<String, (usize, f64)>,
                    track_y: &HashMap<String, f64>|
     -> f64 {
        let mut vals = Vec::new();
        for (k, _) in merged {
            if k.1 == w {
                let v = if let Some(plan) = routing.plans.get(k)
                    && let Some(last) = plan.tracks.last()
                {
                    track_y[last]
                } else {
                    center_of(&k.0, positions)
                };
                vals.push(v);
            }
        }
        if vals.is_empty() {
            MARGIN
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    };

    let mut sorted_watchers = stacks.watchers.clone();
    sorted_watchers.sort_by(|a, b| {
        wanted(a, &positions)
            .partial_cmp(&wanted(b, &positions))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                arriving(a, &positions, &track_y)
                    .partial_cmp(&arriving(b, &positions, &track_y))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut cursor_y = MARGIN + BAND_HEADER_HEIGHT;
    for w in sorted_watchers {
        let wy = wanted(&w, &positions) - height_of[&w] / 2.0;
        let top = cursor_y.max(wy);
        positions.insert(w.clone(), (columns.count, top));
        cursor_y = top + height_of[&w] + ROW_SPACING;
    }
    let watchers_bottom = (y - 8.0).max(cursor_y - ROW_SPACING + BAND_PADDING);
    let total_y = y.max(watchers_bottom + 8.0);
    Placement {
        bands,
        positions,
        track_y,
        height_of: height_of.clone(),
        y: total_y,
        watchers_bottom,
    }
}

struct Ports {
    out_y: HashMap<EdgeKey, f64>,
    in_y: HashMap<EdgeKey, f64>,
    levels: HashMap<EdgeKey, Vec<f64>>,
}

fn place_ports(
    merged: &[(EdgeKey, Vec<AnalyzedCable>)],
    trunk_of: &HashMap<EdgeKey, String>,
    plans: &HashMap<EdgeKey, Plan>,
    placement: &Placement,
) -> Ports {
    let next_y = |k: &EdgeKey| -> f64 {
        if let Some(plan) = plans.get(k)
            && let Some(first) = plan.tracks.first()
        {
            return placement.track_y[first];
        }
        placement.center(&k.1)
    };
    let prev_y = |k: &EdgeKey| -> f64 {
        if let Some(plan) = plans.get(k)
            && let Some(last) = plan.tracks.last()
        {
            return placement.track_y[last];
        }
        placement.center(&k.0)
    };
    let side_loop = |k: &EdgeKey| !plans.contains_key(k);
    let is_send =
        |k: &EdgeKey| side_loop(k) && placement.positions[&k.1].1 > placement.positions[&k.0].1;
    let is_return = |k: &EdgeKey| side_loop(k) && !is_send(k);

    let mut sends_into: HashMap<&str, Vec<&EdgeKey>> = HashMap::new();
    let mut returns_from: HashMap<&str, Vec<&EdgeKey>> = HashMap::new();
    let mut boxes_set = BTreeSet::new();
    for (k, _) in merged {
        boxes_set.insert(k.0.as_str());
        boxes_set.insert(k.1.as_str());
        if is_send(k) {
            sends_into.entry(k.1.as_str()).or_default().push(k);
        }
        if is_return(k) {
            returns_from.entry(k.0.as_str()).or_default().push(k);
        }
    }

    let mut out_y = HashMap::new();
    let mut in_y = HashMap::new();
    for box_key in boxes_set {
        let top = placement.positions[box_key].1;
        let height = placement.height_of[box_key];

        // Right side: sends arriving first, then outgoing trunks.
        let mut right: Vec<Vec<EdgeKey>> = sends_into
            .get(box_key)
            .map(|list| list.iter().map(|&k| vec![k.clone()]).collect())
            .unwrap_or_default();
        let mut trunks: BTreeMap<&str, Vec<EdgeKey>> = BTreeMap::new();
        for (k, _) in merged {
            if k.0 == box_key && !is_return(k) {
                trunks
                    .entry(trunk_of[k].as_str())
                    .or_default()
                    .push(k.clone());
            }
        }
        let mut trunk_groups: Vec<Vec<EdgeKey>> = trunks.into_values().collect();
        trunk_groups.sort_by(|a, b| {
            let a_self = usize::from(a.iter().any(|k| k.0 == k.1));
            let b_self = usize::from(b.iter().any(|k| k.0 == k.1));
            let avg_a = a.iter().map(next_y).sum::<f64>() / a.len() as f64;
            let avg_b = b.iter().map(next_y).sum::<f64>() / b.len() as f64;
            a_self
                .cmp(&b_self)
                .then_with(|| {
                    avg_a
                        .partial_cmp(&avg_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a[0].2.cmp(&b[0].2))
        });
        right.extend(trunk_groups);

        // Left side: returns leaving first, then incoming cables.
        let mut left: Vec<Vec<EdgeKey>> = returns_from
            .get(box_key)
            .map(|list| list.iter().map(|&k| vec![k.clone()]).collect())
            .unwrap_or_default();
        let mut incoming: Vec<EdgeKey> = merged
            .iter()
            .filter(|(k, _)| k.1 == box_key && !is_send(k))
            .map(|(k, _)| k.clone())
            .collect();
        incoming.sort_by(|a, b| {
            let a_self = usize::from(a.0 == a.1);
            let b_self = usize::from(b.0 == b.1);
            a_self
                .cmp(&b_self)
                .then_with(|| {
                    prev_y(a)
                        .partial_cmp(&prev_y(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.2.cmp(&b.2))
        });
        left.extend(incoming.into_iter().map(|k| vec![k]));

        let spread = |slots: &[Vec<EdgeKey>], target: &mut HashMap<EdgeKey, f64>| {
            for (i, slot) in slots.iter().enumerate() {
                let y = top + (height * (i + 1) as f64 / (slots.len() + 1) as f64);
                for k in slot {
                    target.insert(k.clone(), y);
                }
            }
        };
        let mut right_y = HashMap::new();
        let mut left_y = HashMap::new();
        spread(&right, &mut right_y);
        spread(&left, &mut left_y);

        for (k, y) in right_y {
            if k.1 == box_key && is_send(&k) {
                in_y.insert(k, y);
            } else {
                out_y.insert(k, y);
            }
        }
        for (k, y) in left_y {
            if k.0 == box_key && is_return(&k) {
                out_y.insert(k, y);
            } else {
                in_y.insert(k, y);
            }
        }
    }

    let levels = plans
        .iter()
        .map(|(k, plan)| {
            let mut ys = vec![out_y[k]];
            for t in &plan.tracks {
                ys.push(placement.track_y[t]);
            }
            ys.push(in_y[k]);
            (k.clone(), ys)
        })
        .collect();

    Ports {
        out_y,
        in_y,
        levels,
    }
}

fn find_turns(plans: &HashMap<EdgeKey, Plan>, levels: &HashMap<EdgeKey, Vec<f64>>) -> Vec<Turn> {
    let mut turns = Vec::new();
    for (key, plan) in plans {
        let ys = &levels[key];
        for (i, &gap) in plan.gaps.iter().enumerate() {
            let u_turn = plan.feedback && (i == 0 || i + 1 == plan.gaps.len());
            if u_turn || (ys[i + 1] - ys[i]).abs() >= 1.0 {
                turns.push(Turn {
                    gap,
                    key: key.clone(),
                    part: i,
                    y_from: ys[i],
                    y_to: ys[i + 1],
                    feedback: plan.feedback,
                });
            }
        }
    }
    turns
}

fn side_loop_turns(
    merged: &[(EdgeKey, Vec<AnalyzedCable>)],
    plans: &HashMap<EdgeKey, Plan>,
    ports: &Ports,
    positions: &HashMap<String, (usize, f64)>,
) -> Vec<Turn> {
    let mut turns = Vec::new();
    for (k, _) in merged {
        if plans.contains_key(k) {
            continue;
        }
        let col = positions[&k.0].0 as i32;
        let send = positions[&k.1].1 > positions[&k.0].1;
        turns.push(Turn {
            gap: if send { col } else { col - 1 },
            key: k.clone(),
            part: 0,
            y_from: ports.out_y[k],
            y_to: ports.in_y[k],
            feedback: false,
        });
    }
    turns
}

struct Tabs {
    in_tabs: HashMap<EdgeKey, PortTab>,
    out_tabs: HashMap<EdgeKey, PortTab>,
    in_widths: HashMap<usize, f64>,
    out_widths: HashMap<usize, f64>,
    arriving_at: HashMap<String, Vec<PortTab>>,
    leaving_from: HashMap<String, Vec<PortTab>>,
}

impl Tabs {
    fn new(
        merged: &[(EdgeKey, Vec<AnalyzedCable>)],
        plans: &HashMap<EdgeKey, Plan>,
        ports: &Ports,
        enabled: bool,
        positions: &HashMap<String, (usize, f64)>,
    ) -> Self {
        let mut in_tabs = HashMap::new();
        let mut out_tabs = HashMap::new();
        let mut in_widths: HashMap<usize, f64> = HashMap::new();
        let mut out_widths: HashMap<usize, f64> = HashMap::new();
        let mut arriving_at: HashMap<String, Vec<PortTab>> = HashMap::new();
        let mut leaving_from: HashMap<String, Vec<PortTab>> = HashMap::new();

        if enabled {
            for (k, cables) in merged {
                let side_loop = !plans.contains_key(k);
                let send = positions[&k.1].1 > positions[&k.0].1;
                if !side_loop || !send {
                    let arriving_names: Vec<&str> =
                        cables.iter().map(|c| c.to_port.as_str()).collect();
                    let text = port_tab_text(&arriving_names, k.2);
                    let width = port_tab_width(&text);
                    let tip = unique_join(&arriving_names, "\n");
                    let tab = PortTab {
                        output: false,
                        y: ports.in_y[k],
                        width,
                        text,
                        signal: k.2,
                        tooltip: tip,
                    };
                    let col = positions[&k.1].0;
                    in_widths
                        .entry(col)
                        .and_modify(|w| *w = w.max(width))
                        .or_insert(width);
                    arriving_at
                        .entry(k.1.clone())
                        .or_default()
                        .push(tab.clone());
                    in_tabs.insert(k.clone(), tab);
                }
                if !side_loop || send {
                    let leaving_names: Vec<&str> =
                        cables.iter().map(|c| c.from_port.as_str()).collect();
                    let text = port_tab_text(&leaving_names, k.2);
                    let width = port_tab_width(&text);
                    let tip = unique_join(&leaving_names, "\n");
                    let tab = PortTab {
                        output: true,
                        y: ports.out_y[k],
                        width,
                        text,
                        signal: k.2,
                        tooltip: tip,
                    };
                    let col = positions[&k.0].0;
                    out_widths
                        .entry(col)
                        .and_modify(|w| *w = w.max(width))
                        .or_insert(width);
                    leaving_from
                        .entry(k.0.clone())
                        .or_default()
                        .push(tab.clone());
                    out_tabs.insert(k.clone(), tab);
                }
            }
        }

        Self {
            in_tabs,
            out_tabs,
            in_widths,
            out_widths,
            arriving_at,
            leaving_from,
        }
    }

    fn in_width(&self, col: usize) -> f64 {
        self.in_widths.get(&col).copied().unwrap_or(0.0)
    }

    fn out_width(&self, col: usize) -> f64 {
        self.out_widths.get(&col).copied().unwrap_or(0.0)
    }

    fn in_tab_width(&self, k: &EdgeKey) -> f64 {
        self.in_tabs.get(k).map_or(0.0, |t| t.width)
    }

    fn out_tab_width(&self, k: &EdgeKey) -> f64 {
        self.out_tabs.get(k).map_or(0.0, |t| t.width)
    }

    fn of(&self, node_key: &str) -> Vec<PortTab> {
        let mut list = self.arriving_at.get(node_key).cloned().unwrap_or_default();
        if let Some(outs) = self.leaving_from.get(node_key) {
            let mut seen_y = HashSet::new();
            for t in outs {
                if seen_y.insert(t.y.to_bits()) {
                    list.push(t.clone());
                }
            }
        }
        list
    }
}

struct Gaps {
    column_x: Vec<f64>,
    channels: HashMap<(EdgeKey, usize), f64>,
    run_count: HashMap<i32, usize>,
    out_widths: Vec<f64>,
    in_widths: Vec<f64>,
    drawn_columns: usize,
}

impl Gaps {
    fn new<'a>(
        turns: &[Turn],
        tabs: &Tabs,
        drawn_columns: usize,
        trunk_of: impl Fn(&Turn) -> Option<&'a str>,
    ) -> Self {
        let mut by_gap: BTreeMap<i32, Vec<&Turn>> = BTreeMap::new();
        for t in turns {
            by_gap.entry(t.gap).or_default().push(t);
        }
        let mut run_count = HashMap::new();
        for (&gap, list) in &by_gap {
            run_count.insert(gap, bundle_turns(list, &trunk_of).len());
        }
        let in_widths: Vec<f64> = (0..=drawn_columns).map(|c| tabs.in_width(c)).collect();
        let out_widths: Vec<f64> = (0..=drawn_columns).map(|c| tabs.out_width(c)).collect();

        let width_of_gap = |gap: i32| -> f64 {
            let count = run_count.get(&gap).copied().unwrap_or(0);
            let out_w = usize::try_from(gap)
                .ok()
                .and_then(|c| out_widths.get(c).copied())
                .unwrap_or(0.0);
            let in_w = usize::try_from(gap + 1)
                .ok()
                .and_then(|c| in_widths.get(c).copied())
                .unwrap_or(0.0);
            let needed = GAP_PADDING + (count as f64 * CHANNEL_PITCH) + out_w + in_w;
            if gap < 0 || gap >= drawn_columns as i32 - 1 {
                if count > 0 { needed } else { 0.0 }
            } else {
                MIN_GAP.max(needed)
            }
        };

        let mut column_x = vec![MARGIN + width_of_gap(-1)];
        for gap in 0..drawn_columns.saturating_sub(1) {
            let next = column_x[gap] + NODE_WIDTH + width_of_gap(gap as i32);
            column_x.push(next);
        }

        let left_of = |gap: i32| -> f64 {
            if gap < 0 {
                MARGIN
            } else {
                column_x[gap as usize] + NODE_WIDTH
            }
        };
        let right_of = |gap: i32| -> f64 {
            if (gap + 1) < drawn_columns as i32 {
                column_x[(gap + 1) as usize]
            } else {
                left_of(gap) + width_of_gap(gap)
            }
        };

        let mut channels = HashMap::new();
        for (gap, gap_turns) in by_gap {
            let mut ordered = bundle_turns(&gap_turns, &trunk_of);
            let down = |b: &[&Turn]| b[0].y_to > b[0].y_from;
            ordered.sort_by(|a, b| {
                let da = down(a);
                let db = down(b);
                if da != db {
                    return db.cmp(&da); // down (true -> 0) before up
                }
                if da {
                    let max_a = a.iter().map(|t| t.y_to).fold(f64::NEG_INFINITY, f64::max);
                    let max_b = b.iter().map(|t| t.y_to).fold(f64::NEG_INFINITY, f64::max);
                    max_b
                        .partial_cmp(&max_a)
                        .unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    let min_a = a.iter().map(|t| t.y_to).fold(f64::INFINITY, f64::min);
                    let min_b = b.iter().map(|t| t.y_to).fold(f64::INFINITY, f64::min);
                    min_a
                        .partial_cmp(&min_b)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }
            });
            turn_first_when_alongside(&mut ordered);
            let is_self = |b: &[&Turn]| b.iter().all(|t| t.key.0 == t.key.1);
            let mut self_right = Vec::new();
            let mut middle = Vec::new();
            let mut self_left = Vec::new();
            for b in ordered {
                if is_self(&b) && b[0].part == 0 {
                    self_right.push(b);
                } else if is_self(&b) {
                    self_left.push(b);
                } else {
                    middle.push(b);
                }
            }
            ordered = self_right;
            ordered.extend(middle);
            ordered.extend(self_left);

            let out_w = usize::try_from(gap)
                .ok()
                .and_then(|c| out_widths.get(c).copied())
                .unwrap_or(0.0);
            let in_w = usize::try_from(gap + 1)
                .ok()
                .and_then(|c| in_widths.get(c).copied())
                .unwrap_or(0.0);
            let left = left_of(gap) + out_w + 14.0;
            let right = right_of(gap) - in_w - ARROW_LENGTH - 12.0;
            let step = (right - left) / ordered.len().max(1) as f64;
            for (i, bundle) in ordered.into_iter().enumerate() {
                let cx = left + (i as f64 + 0.5) * step;
                for turn in bundle {
                    channels.insert((turn.key.clone(), turn.part), cx);
                }
            }
        }

        Self {
            column_x,
            channels,
            run_count,
            out_widths,
            in_widths,
            drawn_columns,
        }
    }

    fn left(&self, gap: i32) -> f64 {
        if gap < 0 {
            MARGIN
        } else {
            self.column_x[gap as usize] + NODE_WIDTH
        }
    }

    fn right(&self, gap: usize) -> f64 {
        if gap + 1 < self.drawn_columns {
            self.column_x[gap + 1]
        } else {
            let count = self.run_count.get(&(gap as i32)).copied().unwrap_or(0);
            let out_w = self.out_widths.get(gap).copied().unwrap_or(0.0);
            let in_w = self.in_widths.get(gap + 1).copied().unwrap_or(0.0);
            let w = if count > 0 {
                GAP_PADDING + (count as f64 * CHANNEL_PITCH) + out_w + in_w
            } else {
                0.0
            };
            self.left(gap as i32) + w
        }
    }
}

fn bundle_turns<'t, 'a>(
    turns: &[&'t Turn],
    trunk_of: &impl Fn(&Turn) -> Option<&'a str>,
) -> Vec<Vec<&'t Turn>> {
    let mut map: HashMap<String, Vec<&'t Turn>> = HashMap::new();
    let mut order = Vec::new();
    for &t in turns {
        let key = if let Some(trunk) = trunk_of(t) {
            let dir = if t.y_to > t.y_from {
                1
            } else if t.y_to < t.y_from {
                -1
            } else {
                0
            };
            format!("{trunk}|{dir}")
        } else {
            format!("{}|{}|{:?}|{}", t.key.0, t.key.1, t.key.2, t.part)
        };
        if !map.contains_key(&key) {
            order.push(key.clone());
        }
        map.entry(key).or_default().push(t);
    }
    order.into_iter().map(|k| map.remove(&k).unwrap()).collect()
}

fn turn_first_when_alongside(ordered: &mut Vec<Vec<&Turn>>) {
    let alongside = |leaving: &[&Turn], entering: &[&Turn]| -> bool {
        leaving.iter().any(|l| {
            !l.feedback
                && entering.iter().any(|e| {
                    let d = (l.y_to - e.y_from).abs();
                    !e.feedback && d > 0.5 && d < MIN_RUN_SPACING
                })
        })
    };
    let n = ordered.len();
    for _ in 0..n {
        let mut found = None;
        'outer: for i in 0..n {
            for j in (i + 1)..n {
                if alongside(&ordered[i], &ordered[j]) {
                    found = Some((i, j));
                    break 'outer;
                }
            }
        }
        let Some((leaving, entering)) = found else {
            return;
        };
        let bundle = ordered.remove(entering);
        ordered.insert(leaving, bundle);
    }
}

struct Watched {
    by_module: HashMap<i64, Vec<String>>,
    by_group: HashMap<String, Vec<String>>,
}

impl Watched {
    fn new(analysis: &PatchAnalysis, folding: &Folding) -> Self {
        let mut by_module: HashMap<i64, Vec<String>> = HashMap::new();
        let mut by_group: HashMap<String, Vec<String>> = HashMap::new();
        for c in &analysis.cables {
            let from_id = c.cable.from.module_id;
            let to_id = c.cable.to.module_id;
            let Some(to_mod) = analysis.module(to_id) else {
                continue;
            };
            if folding.visible.contains(&from_id)
                && !folding.visible.contains(&to_id)
                && to_mod.band == Band::Monitor
            {
                let from_title = analysis
                    .module(from_id)
                    .map_or("module", |m| m.title.as_str());
                let text = format!(
                    "{} '{}' ← {} '{}'",
                    to_mod.title, c.to_port, from_title, c.from_port
                );
                by_module.entry(from_id).or_default().push(text.clone());
                if let Some(&gi) = folding.group_of.get(&from_id) {
                    by_group
                        .entry(folding.groups[gi].key.clone())
                        .or_default()
                        .push(text);
                }
            }
        }
        Self {
            by_module,
            by_group,
        }
    }

    fn of_node(&self, is_folded: bool, group: &str, module_id: Option<i64>) -> Vec<String> {
        if is_folded {
            self.by_group.get(group).cloned().unwrap_or_default()
        } else if let Some(id) = module_id {
            self.by_module.get(&id).cloned().unwrap_or_default()
        } else {
            Vec::new()
        }
    }
}

fn panel_spec_for_module(m: &AnalyzedModule, count: usize) -> ModulePanelSpec {
    let mut knob_positions: Vec<f32> = m
        .module
        .params
        .values()
        .take(6)
        .map(|&v| {
            // Normalize param into 0..1 visually across typical 0..1 or -1..1 ranges.
            if (0.0..=1.0).contains(&v) {
                v as f32
            } else if (-1.0..=1.0).contains(&v) {
                (v as f32 + 1.0) * 0.5
            } else {
                ((v.sin() * 0.5 + 0.5) as f32).clamp(0.05, 0.95)
            }
        })
        .collect();
    if knob_positions.is_empty() {
        knob_positions.push(0.5);
    }
    let port_total = m.input_count + m.output_count;
    let hp_estimate = ((port_total.max(3) + 2).min(24) as u8).max(4);
    ModulePanelSpec {
        plugin: m.plugin.clone(),
        model: m.module.model.clone(),
        catalog_key: m.module.catalog_key(),
        title: m.title.clone(),
        role: m.role,
        function: m.function,
        hp_estimate,
        knob_positions,
        input_count: m.input_count,
        output_count: m.output_count,
        count,
    }
}

fn panel_specs_for_group(group: &PatchGroup, analysis: &PatchAnalysis) -> Vec<ModulePanelSpec> {
    let mut order = Vec::new();
    let mut map: HashMap<String, (usize, usize)> = HashMap::new(); // catalog_key -> (first_member_idx, count)
    for &id in &group.members {
        let Some(m) = analysis.module(id) else {
            continue;
        };
        let key = m.module.catalog_key();
        if let Some(entry) = map.get_mut(&key) {
            entry.1 += 1;
        } else {
            map.insert(key.clone(), (order.len(), 1));
            order.push((m, key));
        }
    }
    order
        .into_iter()
        .take(4)
        .map(|(m, key)| panel_spec_for_module(m, map[&key].1))
        .collect()
}

fn folded_title(group: &PatchGroup, analysis: &PatchAnalysis) -> String {
    if group.band != Band::Voice {
        return group.band.box_title().to_string();
    }
    let first = &analysis.voices[group.voices[0]];
    let sources = if group.count > 1 {
        let first_mod = analysis
            .module(first.module_ids[0])
            .map_or("Voice", |m| m.title.as_str());
        let base = first_mod.split(" #").next().unwrap_or(first_mod);
        format!("{base} ×{}", group.count)
    } else {
        first.sources.clone()
    };
    match &first.pitch_origin {
        Some(origin) => format!("{sources} ← {origin}"),
        None => sources,
    }
}

fn folded_subtitle(group: &PatchGroup, analysis: &PatchAnalysis) -> String {
    let has_loop = analysis
        .loops
        .iter()
        .any(|l| l.iter().all(|id| group.members.contains(id)));
    let loop_hint = if has_loop { " · ↺ feedback" } else { "" };

    if group.band == Band::Voice {
        let first = &analysis.voices[group.voices[0]];
        let processing: Vec<&str> = first
            .module_ids
            .iter()
            .filter_map(|&id| analysis.module(id))
            .filter(|m| m.role != Role::Source)
            .map(|m| m.title.as_str())
            .collect();
        let mods = group
            .members
            .iter()
            .filter_map(|&id| analysis.module(id))
            .filter(|m| matches!(m.band, Band::Modulation | Band::Pitch))
            .count();
        let chain = if processing.is_empty() {
            "direct out".to_string()
        } else {
            processing.join(" → ")
        };
        let with_mods = if mods > 0 {
            format!("{chain} · +{mods} mod")
        } else {
            chain
        };
        format!("{with_mods}{loop_hint}")
    } else {
        let names: Vec<&str> = group
            .members
            .iter()
            .filter_map(|&id| analysis.module(id))
            .map(|m| m.title.as_str())
            .collect();
        format!("{}{loop_hint}", names.join(", "))
    }
}

fn make_nodes(
    analysis: &PatchAnalysis,
    folding: &Folding,
    positions: &HashMap<String, (usize, f64)>,
    height_of: &HashMap<String, f64>,
    column_x: &[f64],
    tabs: &Tabs,
    monitors: MonitorView,
) -> Vec<DiagramNode> {
    let watched = (monitors == MonitorView::Badge).then(|| Watched::new(analysis, folding));
    let mut nodes = Vec::new();
    for group in &folding.groups {
        if !folding.shows_modules(group) {
            let key = group_key(&group.key);
            let Some(&(col, y)) = positions.get(&key) else {
                continue;
            };
            let watchers = watched
                .as_ref()
                .map(|w| w.of_node(true, &group.key, None))
                .unwrap_or_default();
            nodes.push(DiagramNode {
                key: key.clone(),
                module_id: None,
                group: group.key.clone(),
                is_folded: true,
                title: folded_title(group, analysis),
                subtitle: folded_subtitle(group, analysis),
                role: group.role,
                band: group.band,
                is_insert: false,
                x: column_x[col],
                y,
                width: NODE_WIDTH,
                height: height_of[&key],
                watchers,
                function: None,
                panels: panel_specs_for_group(group, analysis),
                tabs: tabs.of(&key),
            });
            continue;
        }
        for &id in &group.members {
            let key = module_key(id);
            let Some(&(col, y)) = positions.get(&key) else {
                continue;
            };
            let m = analysis.module(id).expect("member in analysis");
            let origin = if let Some(host) = m.insert_of {
                format!(
                    "insert of {}",
                    analysis.module(host).map_or("host", |h| h.title.as_str())
                )
            } else if !m.expanders.is_empty() {
                format!("{} + {}", m.plugin, m.expanders.join(", "))
            } else {
                m.plugin.clone()
            };
            let subtitle = format!("{origin} · {}", m.function.label());
            let watchers = watched
                .as_ref()
                .map(|w| w.of_node(false, &group.key, Some(id)))
                .unwrap_or_default();
            nodes.push(DiagramNode {
                key: key.clone(),
                module_id: Some(id),
                group: group.key.clone(),
                is_folded: false,
                title: m.title.clone(),
                subtitle,
                role: m.role,
                band: m.band,
                is_insert: m.insert_of.is_some(),
                x: column_x[col],
                y,
                width: NODE_WIDTH,
                height: height_of[&key],
                watchers,
                function: Some(m.function),
                panels: vec![panel_spec_for_module(m, 1)],
                tabs: tabs.of(&key),
            });
        }
    }
    nodes
}

#[allow(clippy::too_many_arguments)]
fn build_single_node_for_strip(
    analysis: &PatchAnalysis,
    folding: &Folding,
    box_key: &str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    watched: &Watched,
) -> DiagramNode {
    if let Some(gk) = box_key.strip_prefix("g:")
        && let Some(group) = folding.groups.iter().find(|g| g.key == gk)
    {
        return DiagramNode {
            key: box_key.to_string(),
            module_id: None,
            group: group.key.clone(),
            is_folded: true,
            title: folded_title(group, analysis),
            subtitle: folded_subtitle(group, analysis),
            role: group.role,
            band: group.band,
            is_insert: false,
            x,
            y,
            width,
            height,
            watchers: watched.of_node(true, &group.key, None),
            function: None,
            panels: panel_specs_for_group(group, analysis),
            tabs: Vec::new(),
        };
    }

    let id = module_id_of(box_key).unwrap_or(0);
    let m = analysis.module(id).expect("module in strip");
    let group_key = folding
        .group_of
        .get(&id)
        .map(|&gi| folding.groups[gi].key.as_str())
        .unwrap_or("");
    DiagramNode {
        key: box_key.to_string(),
        module_id: Some(id),
        group: group_key.to_string(),
        is_folded: false,
        title: m.title.clone(),
        subtitle: format!("{} · {}", m.plugin, m.function.label()),
        role: m.role,
        band: m.band,
        is_insert: m.insert_of.is_some(),
        x,
        y,
        width,
        height,
        watchers: watched.of_node(false, group_key, Some(id)),
        function: Some(m.function),
        panels: vec![panel_spec_for_module(m, 1)],
        tabs: Vec::new(),
    }
}

#[derive(Clone, Copy)]
struct LabelRun {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

#[allow(clippy::too_many_arguments)]
fn draw_edges(
    analysis: &PatchAnalysis,
    merged: &[(EdgeKey, Vec<AnalyzedCable>)],
    node_by_key: &HashMap<String, &DiagramNode>,
    plans: &HashMap<EdgeKey, Plan>,
    levels: &HashMap<EdgeKey, Vec<f64>>,
    out_y: &HashMap<EdgeKey, f64>,
    in_y: &HashMap<EdgeKey, f64>,
    channels: &HashMap<(EdgeKey, usize), f64>,
    runs: &mut HashMap<String, Vec<LabelRun>>,
    out_tab: impl Fn(&EdgeKey) -> f64,
    in_tab: impl Fn(&EdgeKey) -> f64,
) -> Vec<DiagramEdge> {
    let mut edges = Vec::new();
    for (key, cables) in merged {
        let from = node_by_key[&key.0];
        let to = node_by_key[&key.1];
        let x1 = from.x + from.width + out_tab(key);
        let y1 = out_y[key];
        let x2 = to.x - in_tab(key) - ARROW_LENGTH;
        let y2 = in_y[key];

        let mut path = String::new();
        let mut label_x = (x1 + x2) / 2.0;
        let mut label_y = y1;
        let mut start_x = x1;
        let mut end_x = x2;
        let mut end_dir = 1.0;
        let mut feedback = false;

        let edge_id = format!("{}>{}:{:?}", key.0, key.1, key.2);
        if let Some(plan) = plans.get(key) {
            let ys = &levels[key];
            feedback = plan.feedback;
            let _ = write!(path, "M {x1:.1} {y1:.1}");
            let mut used = Vec::new();
            let mut corners = vec![(x1, y1)];
            for part in 0..plan.gaps.len() {
                if let Some(&ch) = channels.get(&(key.clone(), part)) {
                    let arriving = if !feedback || part == 0 { 1.0 } else { -1.0 };
                    let leaving = if !feedback || part + 1 == plan.gaps.len() {
                        1.0
                    } else {
                        -1.0
                    };
                    append_bend(&mut path, ch, ys[part], ys[part + 1], arriving, leaving);
                    used.push(ch);
                    corners.push((ch, ys[part]));
                    corners.push((ch, ys[part + 1]));
                    label_x = ch;
                    label_y = (ys[part] + ys[part + 1]) / 2.0;
                }
            }
            let _ = write!(path, " H {x2:.1}");
            corners.push((x2, y2));
            if feedback && !used.is_empty() {
                label_x = (used[0] + used[used.len() - 1]) / 2.0;
                label_y = ys[1];
            } else if !feedback {
                let segs: Vec<LabelRun> = corners
                    .windows(2)
                    .filter(|w| w[0] != w[1])
                    .map(|w| LabelRun {
                        x1: w[0].0,
                        y1: w[0].1,
                        x2: w[1].0,
                        y2: w[1].1,
                    })
                    .collect();
                runs.insert(edge_id.clone(), segs);
            }
        } else {
            let down = to.y > from.y;
            let sx = if down {
                from.x + from.width + out_tab(key)
            } else {
                from.x
            };
            let ex = if down {
                to.x + to.width + ARROW_LENGTH
            } else {
                to.x - in_tab(key) - ARROW_LENGTH
            };
            start_x = sx;
            end_x = ex;
            end_dir = if down { -1.0 } else { 1.0 };
            let outward = if down { 1.0 } else { -1.0 };
            let ch = channels[&(key.clone(), 0)];
            let _ = write!(path, "M {sx:.1} {y1:.1}");
            append_bend(&mut path, ch, y1, y2, outward, -outward);
            let _ = write!(path, " H {ex:.1}");
            label_x = ch;
            label_y = (y1 + y2) / 2.0;
        }

        let first = &cables[0];
        let intent = if cables.len() == 1 {
            first.suggested_intent.clone()
        } else {
            format!("{} × {}", cables.len(), key.2.description())
        };
        let from_port = if cables.len() == 1 {
            first.from_port.clone()
        } else {
            let parts: Vec<&str> = cables.iter().map(|c| c.from_port.as_str()).collect();
            unique_join(&parts, ", ")
        };
        let to_port = if cables.len() == 1 {
            first.to_port.clone()
        } else {
            let mut items = Vec::new();
            for c in cables {
                let s = format!(
                    "{} '{}'",
                    analysis
                        .module(c.cable.to.module_id)
                        .map_or("?", |m| m.title.as_str()),
                    c.to_port
                );
                if !items.contains(&s) {
                    items.push(s);
                }
            }
            items.join(", ")
        };

        edges.push(DiagramEdge {
            key: edge_id,
            from: from.key.clone(),
            to: to.key.clone(),
            cable_ids: cables.iter().map(|c| c.cable.id).collect(),
            from_title: from
                .module_id
                .and_then(|id| analysis.module(id))
                .map_or_else(|| from.title.clone(), |m| m.title.clone()),
            from_port,
            to_title: to
                .module_id
                .and_then(|id| analysis.module(id))
                .map_or_else(|| to.title.clone(), |m| m.title.clone()),
            to_port,
            signal: key.2,
            intent,
            path,
            start_x,
            start_y: y1,
            end_x,
            end_y: y2,
            end_dir,
            label_x,
            label_y,
            is_feedback: feedback,
            is_monitor: to.band == Band::Monitor,
        });
    }
    edges
}

fn append_bend(out: &mut String, x: f64, from_y: f64, to_y: f64, arriving: f64, leaving: f64) {
    let dy = to_y - from_y;
    let sign = if dy > 0.0 {
        1.0
    } else if dy < 0.0 {
        -1.0
    } else {
        0.0
    };
    let r = (dy.abs() / 2.0).min(8.0);
    let _ = write!(
        out,
        " H {:.1} Q {:.1} {:.1} {:.1} {:.1} V {:.1} Q {:.1} {:.1} {:.1} {:.1}",
        x - arriving * r,
        x,
        from_y,
        x,
        from_y + sign * r,
        to_y - sign * r,
        x,
        to_y,
        x + leaving * r,
        to_y
    );
}

// =========================================================================
// Port tab abbreviation & label placement
// =========================================================================

fn port_tab_text(ports: &[&str], signal: SignalType) -> String {
    let mut shorts = Vec::new();
    for &p in ports {
        let s = shorten_port_name(p, signal);
        if !shorts.contains(&s) {
            shorts.push(s);
        }
    }
    let paired = merge_stereo_pairs(&shorts);
    let joined = paired.join(", ");
    if paired.len() == 1 || joined.chars().count() <= 12 {
        joined
    } else {
        format!("{} × {}", shorts.len(), signal.short_label())
    }
}

fn merge_stereo_pairs(shorts: &[String]) -> Vec<String> {
    fn replace_lr(s: &str) -> Option<(String, &str)> {
        let bytes = s.as_bytes();
        for i in 0..bytes.len() {
            if (bytes[i] == b'L' || bytes[i] == b'R')
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
                && (i + 1 == bytes.len() || !bytes[i + 1].is_ascii_alphanumeric())
            {
                let mut both = String::with_capacity(s.len() + 2);
                both.push_str(&s[..i]);
                both.push_str("L/R");
                both.push_str(&s[i + 1..]);
                return Some((both, &s[i..=i]));
            }
        }
        None
    }
    let mut groups: HashMap<String, HashSet<String>> = HashMap::new();
    for s in shorts {
        if let Some((both, side)) = replace_lr(s) {
            groups.entry(both).or_default().insert(side.to_string());
        }
    }
    let mut out = Vec::new();
    for s in shorts {
        let mapped = if let Some((both, _)) = replace_lr(s)
            && groups.get(&both).is_some_and(|sides| sides.len() == 2)
        {
            both
        } else {
            s.clone()
        };
        if !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    out
}

fn port_tab_width(text: &str) -> f64 {
    (text.chars().count() as f64 * 5.4) + 8.0
}

fn shorten_port_name(port: &str, signal: SignalType) -> String {
    const ABBRS: [(&str, &str); 16] = [
        ("Frequency", "Freq"),
        ("Resonance", "Res"),
        ("Modulation", "Mod"),
        ("Envelope", "Env"),
        ("Retrigger", "Retrig"),
        ("Channel", "Ch"),
        ("Input", "In"),
        ("Output", "Out"),
        ("Left", "L"),
        ("Right", "R"),
        ("Position", "Pos"),
        ("Amount", "Amt"),
        ("Attack", "Att"),
        ("Release", "Rel"),
        ("Sustain", "Sus"),
        ("Trigger", "Trig"),
    ];
    let last_chevron = port.rsplit(" › ").next().unwrap_or(port);
    let last_dot = last_chevron.rsplit(" · ").next().unwrap_or(last_chevron);
    let head = last_dot.split(" → ").next().unwrap_or(last_dot);
    let stripped = strip_parens(head);
    let lower = stripped.to_ascii_lowercase();
    if signal == SignalType::Pitch
        && (lower.contains("v/oct") || lower.contains("v / oct") || lower.contains("pitch"))
    {
        return "V/oct".to_string();
    }
    let mut name = stripped;
    for (word, short_word) in ABBRS {
        name = replace_whole_word_ci(&name, word, short_word);
    }
    if name.chars().count() <= 10 {
        name
    } else {
        let truncated: String = name.chars().take(9).collect();
        format!("{}…", truncated.trim_end())
    }
}

fn strip_parens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for ch in s.chars() {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' && depth > 0 {
            depth -= 1;
        } else if depth == 0 {
            out.push(ch);
        }
    }
    out.trim().to_string()
}

fn replace_whole_word_ci(text: &str, word: &str, replacement: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let lower_w = word.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&lower_w) {
        let abs = i + pos;
        let end = abs + word.len();
        let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
        let after_ok = end == text.len() || !text.as_bytes()[end].is_ascii_alphanumeric();
        out.push_str(&text[i..abs]);
        if before_ok && after_ok {
            out.push_str(replacement);
        } else {
            out.push_str(&text[abs..end]);
        }
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

fn unique_join(parts: &[&str], sep: &str) -> String {
    let mut seen = Vec::new();
    for &p in parts {
        if !seen.contains(&p) {
            seen.push(p);
        }
    }
    seen.join(sep)
}

#[derive(Clone, Copy)]
struct BoxBounds {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl BoxBounds {
    fn overlap(self, other: Self) -> f64 {
        let w = (self.right.min(other.right) - self.left.max(other.left)).max(0.0);
        let h = (self.bottom.min(other.bottom) - self.top.max(other.top)).max(0.0);
        w * h
    }
}

fn label_box(text: &str, x: f64, y: f64) -> BoxBounds {
    let half = (text.chars().count() as f64 * 5.8 / 2.0) + 4.0;
    BoxBounds {
        left: x - half,
        top: y - 15.0,
        right: x + half,
        bottom: y,
    }
}

fn place_edge_labels(
    mut edges: Vec<DiagramEdge>,
    runs: &HashMap<String, Vec<LabelRun>>,
    nodes: &[DiagramNode],
) -> Vec<DiagramEdge> {
    let boxes: Vec<BoxBounds> = nodes
        .iter()
        .map(|n| BoxBounds {
            left: n.x - n.in_tabs_width() - ARROW_LENGTH,
            top: n.y,
            right: n.x + n.width + n.out_tabs_width(),
            bottom: n.y + n.height,
        })
        .collect();

    let mut labels = Vec::new();
    let mut movable_indices = Vec::new();
    for (idx, e) in edges.iter().enumerate() {
        let label = e.label();
        if !e.intent.is_empty() && runs.contains_key(&e.key) {
            movable_indices.push(idx);
        } else if !e.intent.is_empty() {
            labels.push(label_box(&label, e.label_x, e.label_y));
        }
    }
    movable_indices.sort_by_key(|&i| std::cmp::Reverse(edges[i].label().len()));

    for idx in movable_indices {
        let e = &edges[idx];
        let label = e.label();
        let half = (label.chars().count() as f64 * 5.8 / 2.0) + 4.0;
        let run_list = &runs[&e.key];
        let mut candidates = vec![(e.label_x, e.label_y)];
        let along = |r: &LabelRun, f: f64| (r.x1 + (r.x2 - r.x1) * f, r.y1 + (r.y2 - r.y1) * f);
        for r in run_list {
            if (r.x1 - r.x2).abs() < 0.1
                || ((r.y1 - r.y2).abs() < 0.1 && (r.x2 - r.x1).abs() >= 2.0 * half + 12.0)
            {
                candidates.push(along(r, 0.5));
            }
        }
        for r in run_list {
            if (r.x1 - r.x2).abs() < 0.1 && (r.y2 - r.y1).abs() > 48.0 {
                candidates.push(along(r, 0.25));
                candidates.push(along(r, 0.75));
            }
        }
        for r in run_list {
            if (r.y1 - r.y2).abs() < 0.1 && (r.x2 - r.x1).abs() >= 2.0 * half + 12.0 {
                let l = r.x1.min(r.x2);
                let rt = r.x1.max(r.x2);
                candidates.push((l + half + 12.0, r.y1));
                candidates.push((rt - half - 12.0, r.y1));
            }
        }

        let cboxes: Vec<BoxBounds> = candidates
            .iter()
            .map(|&(cx, cy)| label_box(&label, cx, cy))
            .collect();
        let best_idx = {
            let mut best = 0;
            let mut best_score = f64::INFINITY;
            for (ci, &cb) in cboxes.iter().enumerate() {
                let on_l: f64 = labels.iter().map(|l| l.overlap(cb)).sum();
                let on_b: f64 = boxes.iter().map(|b| b.overlap(cb)).sum();
                if on_l == 0.0 && on_b == 0.0 {
                    best = ci;
                    break;
                }
                let score = 3.0 * on_l + on_b;
                if score < best_score {
                    best_score = score;
                    best = ci;
                }
            }
            best
        };
        let chosen = cboxes[best_idx];
        labels.push(chosen);
        edges[idx].label_x = (chosen.left + chosen.right) / 2.0;
        edges[idx].label_y = chosen.bottom;
    }
    edges
}
