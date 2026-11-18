//! Ranked module lineup strip and VCV Rack rail coordinate layout.

use super::analyze::{Band, PatchAnalysis};
use super::catalog::{ModuleFunction, Role, VcvCatalog};
use super::panels::resolve_panel;
use std::collections::HashMap;

pub const RACK_HP_WIDTH: f64 = 15.0;
pub const RACK_ROW_HEIGHT: f64 = 380.0;
pub const RACK_ROW_GAP: f64 = 12.0;

/// One deduplicated module shown on the compact ranked list strip.
#[derive(Clone, Debug)]
pub struct RankedStripModule {
    pub plugin: String,
    pub model: String,
    pub title: String,
    pub role: Role,
    pub function: ModuleFunction,
    pub count: usize,
    pub cable_degree: usize,
    pub importance_tier: u8,
    pub hp: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Horizontal ranked module lineup strip (no boxes, no cables).
#[derive(Clone, Debug)]
pub struct StripLineup {
    pub width: f64,
    pub height: f64,
    pub modules: Vec<RankedStripModule>,
}

/// One module placed at its real Eurorack coordinates on a rail.
#[derive(Clone, Debug)]
pub struct PlacedRackModule {
    pub id: i64,
    pub plugin: String,
    pub model: String,
    pub title: String,
    pub role: Role,
    pub function: ModuleFunction,
    pub row: i32,
    pub column_hp: i32,
    pub hp: i32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Multi-row Eurorack rail layout placing modules at their saved `[column_hp, row]` positions.
#[derive(Clone, Debug)]
pub struct RackRailLayout {
    pub width: f64,
    pub height: f64,
    pub min_row: i32,
    pub max_row: i32,
    pub min_hp: i32,
    pub max_hp: i32,
    pub rows: Vec<(i32, f64)>,
    pub modules: Vec<PlacedRackModule>,
}

const STRIP_H: f64 = 128.0;
const STRIP_PANEL_H: f64 = 120.0;
const STRIP_PANEL_Y: f64 = 4.0;
const STRIP_GAP: f64 = 4.0;
const STRIP_MARGIN_X: f64 = 4.0;

struct StripEntry {
    plugin: String,
    model: String,
    title: String,
    role: Role,
    function: ModuleFunction,
    count: usize,
    cable_degree: usize,
    importance_tier: u8,
    first_pos: (i32, i32, i64),
    hp_est: u32,
}

fn collect_strip_entries(analysis: &PatchAnalysis) -> Vec<StripEntry> {
    let mut cable_deg: HashMap<i64, usize> = HashMap::new();
    let mut has_outputs: HashMap<i64, bool> = HashMap::new();
    for c in &analysis.document.cables {
        *cable_deg.entry(c.from.module_id).or_default() += 1;
        *cable_deg.entry(c.to.module_id).or_default() += 1;
        has_outputs.insert(c.from.module_id, true);
    }

    let catalog = VcvCatalog::embedded();
    analysis
        .document
        .modules
        .iter()
        .map(|m| {
            let analyzed = analysis.module(m.id);
            let deg = cable_deg.get(&m.id).copied().unwrap_or(0);
            let outputs = has_outputs.get(&m.id).copied().unwrap_or(false);

            let info = catalog.module_info(&m.plugin, &m.model);
            let (role, function, title) = if let Some(am) = analyzed {
                (am.role, am.function, am.title.clone())
            } else {
                let (r, _) = catalog.resolve_role(&m.plugin, &m.model, info);
                let tags = info.map(|i| i.tags.as_slice()).unwrap_or(&[]);
                let name = info.and_then(|i| i.name.as_deref());
                let f = ModuleFunction::classify(r, tags, &m.model, name);
                (r, f, name.unwrap_or(&m.model).to_string())
            };

            let is_blank = info
                .is_some_and(|i| i.tags.iter().any(|t| t.eq_ignore_ascii_case("Blank")))
                || m.model.to_ascii_lowercase().contains("blank");
            let tier = if is_blank {
                8
            } else if deg == 0 && analyzed.is_none() {
                7
            } else {
                importance_tier(role, function, outputs, analyzed.map(|a| a.band))
            };

            let port_total = info.map_or(4, |i| i.inputs.len() + i.outputs.len());
            let hp_est = ((port_total.max(3) + 2).min(24) as u32).max(4);
            StripEntry {
                plugin: m.plugin.clone(),
                model: m.model.clone(),
                title,
                role,
                function,
                count: 1,
                cable_degree: deg,
                importance_tier: tier,
                first_pos: (m.row, m.column, m.id),
                hp_est,
            }
        })
        .collect()
}

fn pack_strip_lineup(entries: Vec<StripEntry>) -> StripLineup {
    let mut x = STRIP_MARGIN_X;
    let mut modules = Vec::with_capacity(entries.len());
    for item in entries {
        let aspect = resolve_panel(&item.plugin, &item.model)
            .map(|p| p.aspect)
            .unwrap_or_else(|| (f64::from(item.hp_est) * RACK_HP_WIDTH) / RACK_ROW_HEIGHT);
        let width = (STRIP_PANEL_H * aspect).round().clamp(18.0, 220.0);
        let hp = ((aspect * RACK_ROW_HEIGHT) / RACK_HP_WIDTH)
            .round()
            .clamp(2.0, 48.0) as u32;
        modules.push(RankedStripModule {
            plugin: item.plugin,
            model: item.model,
            title: item.title,
            role: item.role,
            function: item.function,
            count: item.count,
            cable_degree: item.cable_degree,
            importance_tier: item.importance_tier,
            hp,
            x,
            y: STRIP_PANEL_Y,
            width,
            height: STRIP_PANEL_H,
        });
        x += width + STRIP_GAP;
    }

    let total_w = if modules.is_empty() {
        160.0
    } else {
        (x - STRIP_GAP + STRIP_MARGIN_X).max(64.0)
    };
    StripLineup {
        width: total_w,
        height: STRIP_H,
        modules,
    }
}

/// Builds the full left-to-right ordered strip (preserving duplicates) when every
/// module resides on a single Eurorack row.
pub fn build_full_single_row_strip(analysis: &PatchAnalysis, _dark: bool) -> Option<StripLineup> {
    let first_row = analysis.document.modules.first()?.row;
    if !analysis
        .document
        .modules
        .iter()
        .all(|module| module.row == first_row)
    {
        return None;
    }
    Some(pack_strip_lineup(collect_strip_entries(analysis)))
}

/// Builds the compact deduplicated strip ordered by musical importance.
pub fn build_ranked_strip(analysis: &PatchAnalysis, _dark: bool) -> StripLineup {
    let mut grouped: HashMap<(String, String), StripEntry> = HashMap::new();
    for item in collect_strip_entries(analysis) {
        let key = (item.plugin.clone(), item.model.clone());
        if let Some(existing) = grouped.get_mut(&key) {
            existing.count += item.count;
            existing.cable_degree += item.cable_degree;
            existing.importance_tier = existing.importance_tier.min(item.importance_tier);
        } else {
            grouped.insert(key, item);
        }
    }

    // Retain every distinct module sorted by musical priority so all modules render
    // whenever horizontal space permits, while narrow rows crop lower tiers on the right.
    let mut list: Vec<StripEntry> = grouped.into_values().collect();
    list.sort_by_key(|item| {
        (
            item.importance_tier,
            std::cmp::Reverse(item.cable_degree),
            std::cmp::Reverse(item.count),
            item.first_pos,
        )
    });
    pack_strip_lineup(list)
}

fn importance_tier(
    role: Role,
    function: ModuleFunction,
    has_outputs: bool,
    band: Option<Band>,
) -> u8 {
    if band == Some(Band::Monitor) || role == Role::Monitor {
        return 6;
    }
    if role == Role::Io && !has_outputs {
        return 6;
    }
    if role == Role::Source
        || function == ModuleFunction::Sequencer
        || role == Role::Performance
        || (role == Role::Io && has_outputs)
    {
        return 0;
    }
    if matches!(
        function,
        ModuleFunction::Filter | ModuleFunction::Waveshaper
    ) {
        return 1;
    }
    if role == Role::Effect {
        return 2;
    }
    if matches!(
        function,
        ModuleFunction::Clock | ModuleFunction::Quantizer | ModuleFunction::PitchUtility
    ) {
        return 3;
    }
    if matches!(
        function,
        ModuleFunction::Envelope
            | ModuleFunction::Lfo
            | ModuleFunction::Random
            | ModuleFunction::Slew
            | ModuleFunction::Modulator
            | ModuleFunction::Controller
    ) {
        return 4;
    }
    5
}

/// Places all patch modules onto Eurorack rails at their actual saved `[column_hp, row]` positions.
pub fn build_rack_rail_layout(analysis: &PatchAnalysis, _dark: bool) -> RackRailLayout {
    const MARGIN: f64 = 16.0;
    let catalog = VcvCatalog::embedded();

    if analysis.document.modules.is_empty() {
        return RackRailLayout {
            width: 320.0,
            height: RACK_ROW_HEIGHT + MARGIN * 2.0,
            min_row: 0,
            max_row: 0,
            min_hp: 0,
            max_hp: 20,
            rows: vec![(0, MARGIN)],
            modules: Vec::new(),
        };
    }

    // Group modules by rack row (already sorted by row, column, id in PatchDocument).
    let mut row_map: std::collections::BTreeMap<i32, Vec<_>> = std::collections::BTreeMap::new();
    for m in &analysis.document.modules {
        row_map.entry(m.row).or_default().push(m);
    }

    struct Draft {
        id: i64,
        plugin: String,
        model: String,
        title: String,
        role: Role,
        function: ModuleFunction,
        row: i32,
        col: i32,
        hp: i32,
    }

    let mut drafts = Vec::with_capacity(analysis.document.modules.len());
    let mut min_hp = i32::MAX;
    let mut max_hp = i32::MIN;

    for (row_idx, mods) in &row_map {
        let mut cursor_hp = i32::MIN;
        for m in mods {
            let analyzed = analysis.module(m.id);
            let info = catalog.module_info(&m.plugin, &m.model);
            let (role, function, title) = if let Some(am) = analyzed {
                (am.role, am.function, am.title.clone())
            } else {
                let (r, _) = catalog.resolve_role(&m.plugin, &m.model, info);
                let tags = info.map(|i| i.tags.as_slice()).unwrap_or(&[]);
                let name = info.and_then(|i| i.name.as_deref());
                let f = ModuleFunction::classify(r, tags, &m.model, name);
                (r, f, name.unwrap_or(&m.model).to_string())
            };

            let port_total = info.map_or(4, |i| i.inputs.len() + i.outputs.len());
            let hp_fallback = ((port_total.max(3) + 2).min(24) as i32).max(4);
            let hp = resolve_panel(&m.plugin, &m.model)
                .map(|p| ((p.aspect * RACK_ROW_HEIGHT) / RACK_HP_WIDTH).round() as i32)
                .unwrap_or(hp_fallback)
                .clamp(2, 64);

            let col = if cursor_hp == i32::MIN {
                m.column
            } else {
                m.column.max(cursor_hp)
            };
            cursor_hp = col + hp;
            min_hp = min_hp.min(col);
            max_hp = max_hp.max(col + hp);

            drafts.push(Draft {
                id: m.id,
                plugin: m.plugin.clone(),
                model: m.model.clone(),
                title,
                role,
                function,
                row: *row_idx,
                col,
                hp,
            });
        }
    }

    let pad_hp = 1;
    let origin_hp = min_hp - pad_hp;
    let total_hp = (max_hp - min_hp + pad_hp * 2).max(16);

    let min_row = *row_map.keys().next().unwrap_or(&0);
    let max_row = *row_map.keys().next_back().unwrap_or(&0);
    let mut rows = Vec::with_capacity(row_map.len());
    let mut row_y_map = HashMap::new();
    for (slot, &row_idx) in row_map.keys().enumerate() {
        let y = MARGIN + slot as f64 * (RACK_ROW_HEIGHT + RACK_ROW_GAP);
        rows.push((row_idx, y));
        row_y_map.insert(row_idx, y);
    }

    let modules = drafts
        .into_iter()
        .map(|d| PlacedRackModule {
            id: d.id,
            plugin: d.plugin,
            model: d.model,
            title: d.title,
            role: d.role,
            function: d.function,
            row: d.row,
            column_hp: d.col,
            hp: d.hp,
            x: MARGIN + (d.col - origin_hp) as f64 * RACK_HP_WIDTH,
            y: row_y_map[&d.row],
            width: d.hp as f64 * RACK_HP_WIDTH,
            height: RACK_ROW_HEIGHT,
        })
        .collect();

    let width = total_hp as f64 * RACK_HP_WIDTH + MARGIN * 2.0;
    let height = rows.len() as f64 * RACK_ROW_HEIGHT
        + rows.len().saturating_sub(1) as f64 * RACK_ROW_GAP
        + MARGIN * 2.0;

    RackRailLayout {
        width,
        height,
        min_row,
        max_row,
        min_hp: origin_hp,
        max_hp: origin_hp + total_hp,
        rows,
        modules,
    }
}
