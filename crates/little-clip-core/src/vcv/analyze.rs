//! Patch topology analysis, signal typing, voice detection, and group folding
//! ported from `ModularPatchTools.Diagram.Analysis`.

use super::archive::{PatchCable, PatchDocument, PatchModule, PortRef};
use super::catalog::{
    ModRoute, ModuleFunction, Role, RoleSource, VcvCatalog, canonical_tags, is_port_labeled,
    resolve_midi_learn_name, resolve_port_label,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Functional signal category of a cable or connection bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SignalType {
    Audio,
    Pitch,
    Cv,
    Gate,
}

impl SignalType {
    pub fn layer_key(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Pitch => "pitch",
            Self::Gate => "gate",
            Self::Cv => "modulation",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Pitch => "pitch",
            Self::Gate => "gate",
            Self::Cv => "CV",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Pitch => "Pitch",
            Self::Gate => "Gate / trig / clock",
            Self::Cv => "CV / modulation",
        }
    }
}

/// Horizontal band of the diagram (top to bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Band {
    External,
    Time,
    Pitch,
    Voice,
    Bus,
    Modulation,
    Monitor,
}

impl Band {
    pub fn box_title(self) -> &'static str {
        match self {
            Self::External => "Played live",
            Self::Time => "Sequencing",
            Self::Pitch => "Pitch processing",
            Self::Modulation => "Shared modulation",
            Self::Voice => "Voice",
            Self::Monitor => "Scopes & displays",
            Self::Bus => "Mix & effects",
        }
    }

    pub fn lane_title(self, shared: bool) -> &'static str {
        match self {
            Self::External => "Performance (MIDI, host, played live)",
            Self::Time => "Time & sequencing",
            Self::Pitch => {
                if shared {
                    "Shared pitch processing"
                } else {
                    "Pitch"
                }
            }
            Self::Modulation => {
                if shared {
                    "Shared modulation"
                } else {
                    "Modulation"
                }
            }
            Self::Voice => "Voices",
            Self::Monitor => "Monitoring (scopes, displays)",
            Self::Bus => "Mix & effects",
        }
    }

    pub fn default_role(self) -> Role {
        match self {
            Self::External => Role::Performance,
            Self::Time => Role::Time,
            Self::Pitch => Role::Pitch,
            Self::Bus => Role::Mixer,
            Self::Monitor => Role::Monitor,
            Self::Voice | Self::Modulation => Role::Controller,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnalyzedModule {
    pub module: PatchModule,
    pub title: String,
    pub plugin: String,
    pub model_name: String,
    pub role: Role,
    pub role_source: RoleSource,
    pub band: Band,
    pub voice: Option<usize>,
    pub insert_of: Option<i64>,
    pub expanders: Vec<String>,
    pub function: ModuleFunction,
    pub input_count: usize,
    pub output_count: usize,
}

#[derive(Clone, Debug)]
pub struct AnalyzedCable {
    pub cable: PatchCable,
    pub from_port: String,
    pub to_port: String,
    pub signal: SignalType,
    pub suggested_intent: String,
    pub routes: Vec<ModRoute>,
}

#[derive(Clone, Debug)]
pub struct Voice {
    pub index: usize,
    pub module_ids: Vec<i64>,
    pub sources: String,
    pub pitch_origin: Option<String>,
}

impl Voice {
    pub fn name(&self) -> String {
        let base = format!("Voice {}: {}", self.index + 1, self.sources);
        match &self.pitch_origin {
            Some(origin) => format!("{base} ← {origin}"),
            None => base,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PatchGroup {
    pub key: String,
    pub band: Band,
    pub title: String,
    pub members: Vec<i64>,
    pub voices: Vec<usize>,
    pub count: usize,
    pub role: Role,
}

/// Complete semantic analysis of a `.vcv` patch.
#[derive(Clone, Debug)]
pub struct PatchAnalysis {
    pub document: PatchDocument,
    pub modules: Vec<AnalyzedModule>,
    pub cables: Vec<AnalyzedCable>,
    pub voices: Vec<Voice>,
    pub loops: Vec<Vec<i64>>,
    pub diagnostics: Vec<String>,
    module_index_by_id: HashMap<i64, usize>,
}

impl PatchAnalysis {
    pub fn module(&self, id: i64) -> Option<&AnalyzedModule> {
        self.module_index_by_id.get(&id).map(|&i| &self.modules[i])
    }

    pub fn cable_count(&self) -> usize {
        self.document.cables.len()
    }

    pub fn shown_module_count(&self) -> usize {
        self.modules.len()
    }

    pub fn monitor_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.band == Band::Monitor)
            .count()
    }
}

/// Analyzes a `PatchDocument` using the embedded `VcvCatalog`.
pub fn analyze_patch(saved: PatchDocument) -> Result<PatchAnalysis, String> {
    saved.validate_preview_limits()?;
    let catalog = VcvCatalog::embedded();
    let patch = bypass_wireless_links(&saved);
    let mut diagnostics = Vec::new();
    let by_id: HashMap<i64, &PatchModule> = patch
        .modules
        .iter()
        .map(|module| (module.id, module))
        .collect();

    let base_of = expander_base_of(&patch.modules, catalog);
    let mut cabled_ids = HashSet::new();
    for c in &patch.cables {
        if by_id.contains_key(&c.from.module_id) && by_id.contains_key(&c.to.module_id) {
            cabled_ids.insert(*base_of.get(&c.from.module_id).unwrap_or(&c.from.module_id));
            cabled_ids.insert(*base_of.get(&c.to.module_id).unwrap_or(&c.to.module_id));
        }
    }

    let shown: Vec<PatchModule> = patch
        .modules
        .iter()
        .filter(|m| cabled_ids.contains(&m.id))
        .cloned()
        .collect();
    let titles = build_titles(&shown, catalog);

    let mut role_by_id: HashMap<i64, Role> = HashMap::new();
    let mut role_source_by_id: HashMap<i64, RoleSource> = HashMap::new();
    for module in &shown {
        let info = catalog.module_info(&module.plugin, &module.model);
        let (role, source) = catalog.resolve_role(&module.plugin, &module.model, info);
        role_by_id.insert(module.id, role);
        role_source_by_id.insert(module.id, source);
        if source >= RoleSource::Description {
            let kind = if source == RoleSource::Description {
                "VCV Library description"
            } else {
                "name"
            };
            diagnostics.push(format!(
                "{}: role guessed as {} from its {kind}",
                titles[&module.id],
                role.label()
            ));
        }
    }

    let cables = type_cables(
        &patch,
        &by_id,
        &base_of,
        &titles,
        &role_by_id,
        catalog,
        &mut diagnostics,
    );
    resolve_contextual_roles(&shown, &cables, &mut role_by_id);

    let mut audio_role = role_by_id.clone();
    for m in &shown {
        if role_by_id[&m.id] == Role::Io
            && cables
                .iter()
                .any(|c| c.cable.from.module_id == m.id && c.signal == SignalType::Audio)
        {
            audio_role.insert(m.id, Role::Source);
        }
    }

    let mut origin_of: HashMap<i64, Option<AnalyzedCable>> = HashMap::new();
    for m in &shown {
        if audio_role[&m.id] == Role::Source {
            origin_of.insert(m.id, pitch_origin(m.id, &cables, &role_by_id).cloned());
        }
    }

    let rack_order: Vec<i64> = shown.iter().map(|m| m.id).collect();
    let topology = build_audio_topology(&rack_order, &cables, &audio_role, &origin_of);

    let mut origin_ports_by_mod: HashMap<i64, HashSet<i32>> = HashMap::new();
    for cable in origin_of.values().flatten() {
        origin_ports_by_mod
            .entry(cable.cable.from.module_id)
            .or_default()
            .insert(cable.cable.from.port_id);
    }
    let split_origins: HashSet<i64> = origin_ports_by_mod
        .into_iter()
        .filter(|(_, ports)| ports.len() > 1)
        .map(|(id, _)| id)
        .collect();

    let origin_name = |origin: &AnalyzedCable| -> String {
        if split_origins.contains(&origin.cable.from.module_id) {
            format!(
                "{} › {}",
                titles[&origin.cable.from.module_id], origin.from_port
            )
        } else {
            titles[&origin.cable.from.module_id].clone()
        }
    };

    let origin_of_voice = |members: &[i64]| -> Option<AnalyzedCable> {
        members
            .iter()
            .filter(|id| audio_role[id] == Role::Source)
            .find_map(|id| origin_of.get(id).and_then(|o| o.clone()))
    };

    // Group voices sharing a split keyboard origin together in output port order.
    type VoiceGroupEntry = (usize, Vec<i64>, Option<AnalyzedCable>);
    let mut voice_groups: BTreeMap<i64, Vec<VoiceGroupEntry>> = BTreeMap::new();
    for (pos, members) in topology.voices.into_iter().enumerate() {
        let origin = origin_of_voice(&members);
        let group_key = match &origin {
            Some(o) if split_origins.contains(&o.cable.from.module_id) => o.cable.from.module_id,
            _ => -1 - (pos as i64),
        };
        voice_groups
            .entry(group_key)
            .or_default()
            .push((pos, members, origin));
    }
    let mut voice_group_list: Vec<_> = voice_groups.into_values().collect();
    voice_group_list.sort_by_key(|g| g.iter().map(|(pos, _, _)| *pos).min().unwrap_or(usize::MAX));
    let mut ordered_voices = Vec::new();
    for mut g in voice_group_list {
        g.sort_by_key(|(pos, _, origin)| {
            (origin.as_ref().map_or(0, |o| o.cable.from.port_id), *pos)
        });
        for (_, members, _) in g {
            ordered_voices.push(members);
        }
    }

    let mut voices = Vec::with_capacity(ordered_voices.len());
    let mut voice_by_id: HashMap<i64, usize> = HashMap::new();
    for (index, members) in ordered_voices.into_iter().enumerate() {
        for &id in &members {
            voice_by_id.insert(id, index);
        }
        let sources: Vec<String> = members
            .iter()
            .filter(|id| audio_role[id] == Role::Source)
            .map(|id| titles[id].clone())
            .collect();
        let origin = origin_of_voice(&members);
        voices.push(Voice {
            index,
            module_ids: members,
            sources: sources.join(" + "),
            pitch_origin: origin.as_ref().map(origin_name),
        });
    }

    // Track actual plugged/catalog port counts for miniature faceplate detail.
    let mut max_in_by_mod: HashMap<i64, usize> = HashMap::new();
    let mut max_out_by_mod: HashMap<i64, usize> = HashMap::new();
    for c in &saved.cables {
        let out_n = usize::try_from(c.from.port_id).map_or(0, |port| port + 1);
        let in_n = usize::try_from(c.to.port_id).map_or(0, |port| port + 1);
        let from_base = *base_of.get(&c.from.module_id).unwrap_or(&c.from.module_id);
        let to_base = *base_of.get(&c.to.module_id).unwrap_or(&c.to.module_id);
        max_out_by_mod
            .entry(from_base)
            .and_modify(|v| *v = (*v).max(out_n))
            .or_insert(out_n);
        max_in_by_mod
            .entry(to_base)
            .and_modify(|v| *v = (*v).max(in_n))
            .or_insert(in_n);
    }

    let mut modules = Vec::with_capacity(shown.len());
    let mut module_index_by_id = HashMap::with_capacity(shown.len());
    for module in shown {
        let host = topology.insert_host.get(&module.id).copied();
        let placed_as = host.unwrap_or(module.id);
        let voice = voice_by_id.get(&placed_as).copied();
        let band = if voice.is_some() {
            Band::Voice
        } else {
            band_for(placed_as, role_by_id[&placed_as], &cables)
        };
        let info = catalog.module_info(&module.plugin, &module.model);
        let tags = info.map(|i| i.tags.as_slice()).unwrap_or(&[]);
        let model_name = info
            .and_then(|i| i.name.as_deref())
            .unwrap_or(&module.model)
            .to_string();
        let function = ModuleFunction::classify(
            role_by_id[&module.id],
            tags,
            &module.model,
            info.and_then(|i| i.name.as_deref()),
        );
        let expanders: Vec<String> = base_of
            .iter()
            .filter_map(|(&exp_id, &b_id)| {
                (b_id == module.id)
                    .then(|| by_id.get(&exp_id).map(|exp| expander_name(exp, catalog)))
                    .flatten()
            })
            .collect();
        let input_count = info
            .map(|i| i.inputs.len())
            .unwrap_or(0)
            .max(*max_in_by_mod.get(&module.id).unwrap_or(&0));
        let output_count = info
            .map(|i| i.outputs.len())
            .unwrap_or(0)
            .max(*max_out_by_mod.get(&module.id).unwrap_or(&0));

        module_index_by_id.insert(module.id, modules.len());
        modules.push(AnalyzedModule {
            title: titles[&module.id].clone(),
            plugin: module.plugin.clone(),
            model_name,
            role: role_by_id[&module.id],
            role_source: role_source_by_id[&module.id],
            band,
            voice,
            insert_of: host,
            expanders,
            function,
            input_count,
            output_count,
            module,
        });
    }

    let host_of: HashMap<i64, i64> = modules
        .iter()
        .filter_map(|m| m.insert_of.map(|h| (m.module.id, h)))
        .collect();
    let loop_host = |id: i64| -> i64 { *host_of.get(&id).unwrap_or(&id) };
    let order_ids: Vec<i64> = modules.iter().map(|m| m.module.id).collect();
    let order_pos: HashMap<i64, usize> = order_ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();
    let loop_edges: Vec<(i64, i64)> = cables
        .iter()
        .filter(|c| loop_host(c.cable.from.module_id) != loop_host(c.cable.to.module_id))
        .map(|c| (c.cable.from.module_id, c.cable.to.module_id))
        .collect();
    let mut loops = graph_loops(&order_ids, &loop_edges);
    let self_patched: HashSet<i64> = cables
        .iter()
        .filter(|c| c.cable.from.module_id == c.cable.to.module_id)
        .map(|c| c.cable.from.module_id)
        .collect();
    for &id in &order_ids {
        if self_patched.contains(&id) {
            loops.push(vec![id]);
        }
    }
    loops.sort_by_key(|l| {
        (
            order_pos.get(&l[0]).copied().unwrap_or(usize::MAX),
            std::cmp::Reverse(l.len()),
        )
    });

    Ok(PatchAnalysis {
        document: saved,
        modules,
        cables,
        voices,
        loops,
        diagnostics,
        module_index_by_id,
    })
}

/// Groups a analyzed patch into foldable logical units (`PatchGroup`).
pub fn build_patch_groups(analysis: &PatchAnalysis) -> Vec<PatchGroup> {
    let mut group_of: HashMap<i64, String> = HashMap::new();
    let mut raw_groups: Vec<(String, Band, String, Vec<usize>, Role)> = Vec::new();

    // Identical 1-module voices fold into one group; multi-module voices keep their own group.
    let mut voice_lanes: Vec<(String, Vec<&Voice>)> = Vec::new();
    let mut lane_index_by_key: HashMap<String, usize> = HashMap::new();
    for v in &analysis.voices {
        let lane_key = if v.module_ids.len() == 1 {
            format!(
                "single:{}",
                analysis
                    .module(v.module_ids[0])
                    .map(|m| m.module.catalog_key())
                    .unwrap_or_default()
            )
        } else {
            format!("voice:{}", v.index)
        };
        if let Some(&idx) = lane_index_by_key.get(&lane_key) {
            voice_lanes[idx].1.push(v);
        } else {
            lane_index_by_key.insert(lane_key.clone(), voice_lanes.len());
            voice_lanes.push((lane_key, vec![v]));
        }
    }

    for (_, voices) in voice_lanes {
        let indices: Vec<usize> = voices.iter().map(|v| v.index).collect();
        let key = format!(
            "voice-{}",
            indices
                .iter()
                .map(|i| (i + 1).to_string())
                .collect::<Vec<_>>()
                .join("-")
        );
        let title = if voices.len() == 1 {
            voices[0].name()
        } else {
            let nums = indices
                .iter()
                .map(|i| (i + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Voices {nums}: {} ×{}",
                base_title(&voices[0].sources),
                voices.len()
            )
        };
        raw_groups.push((
            key.clone(),
            Band::Voice,
            title,
            indices.clone(),
            Role::Source,
        ));
        for m in &analysis.modules {
            if let Some(v) = m.voice
                && indices.contains(&v)
            {
                group_of.insert(m.module.id, key.clone());
            }
        }
    }

    for m in &analysis.modules {
        if m.band == Band::Bus {
            group_of.insert(m.module.id, "bus".to_string());
        }
    }

    // Fold private modulators and pitch processors into the voice (or bus) they serve.
    let candidates: Vec<i64> = analysis
        .modules
        .iter()
        .filter(|m| matches!(m.band, Band::Modulation | Band::Pitch))
        .map(|m| m.module.id)
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for &cand_id in &candidates {
            if group_of.contains_key(&cand_id) {
                continue;
            }
            let mut targets: Vec<Option<String>> = Vec::new();
            for c in &analysis.cables {
                if c.cable.from.module_id != cand_id {
                    continue;
                }
                if analysis
                    .module(c.cable.to.module_id)
                    .is_some_and(|m| m.band == Band::Monitor)
                {
                    continue;
                }
                let target_group = channel_owner(analysis, c, &group_of)
                    .or_else(|| group_of.get(&c.cable.to.module_id).cloned());
                if !targets.contains(&target_group) {
                    targets.push(target_group);
                }
            }
            if targets.len() == 1
                && let Some(Some(owner)) = targets.into_iter().next()
            {
                group_of.insert(cand_id, owner);
                changed = true;
            }
        }
    }

    for m in &analysis.modules {
        if group_of.contains_key(&m.module.id) {
            continue;
        }
        let key = band_key(m.band).to_string();
        group_of.insert(m.module.id, key.clone());
        if !raw_groups.iter().any(|g| g.0 == key) {
            raw_groups.push((
                key,
                m.band,
                m.band.lane_title(true).to_string(),
                Vec::new(),
                m.band.default_role(),
            ));
        }
    }

    if group_of.values().any(|g| g == "bus") {
        raw_groups.push((
            "bus".to_string(),
            Band::Bus,
            Band::Bus.lane_title(false).to_string(),
            Vec::new(),
            Role::Mixer,
        ));
    }

    raw_groups.sort_by_key(|g| g.1);
    raw_groups
        .into_iter()
        .map(|(key, band, title, voices, role)| {
            let members: Vec<i64> = analysis
                .modules
                .iter()
                .filter(|m| group_of.get(&m.module.id) == Some(&key))
                .map(|m| m.module.id)
                .collect();
            let count = voices.len().max(1);
            PatchGroup {
                key,
                band,
                title,
                members,
                voices,
                count,
                role,
            }
        })
        .collect()
}

fn band_key(band: Band) -> &'static str {
    match band {
        Band::External => "external",
        Band::Time => "time",
        Band::Pitch => "pitch",
        Band::Voice => "voice",
        Band::Bus => "bus",
        Band::Modulation => "modulation",
        Band::Monitor => "monitor",
    }
}

fn channel_owner(
    analysis: &PatchAnalysis,
    cable: &AnalyzedCable,
    group_of: &HashMap<i64, String>,
) -> Option<String> {
    let mixer_id = cable.cable.to.module_id;
    let mixer = analysis.module(mixer_id)?;
    if cable.signal == SignalType::Audio || mixer.role != Role::Mixer {
        return None;
    }
    let ch = channel_number(&cable.to_port)?;
    let mut owners = HashSet::new();
    for c in &analysis.cables {
        if c.cable.to.module_id == mixer_id
            && c.signal == SignalType::Audio
            && channel_number(&c.to_port).as_deref() == Some(ch.as_str())
            && let Some(owner) = group_of.get(&c.cable.from.module_id)
        {
            owners.insert(owner.clone());
        }
    }
    if owners.len() == 1 {
        let only = owners.into_iter().next()?;
        if only.starts_with("voice-") {
            return Some(only);
        }
    }
    None
}

fn channel_number(port: &str) -> Option<String> {
    let lower = port.to_ascii_lowercase();
    for prefix in ["channel", "ch", "input", "in"] {
        let mut idx = 0;
        while let Some(pos) = lower[idx..].find(prefix) {
            let abs = idx + pos;
            let before_ok = abs == 0 || !lower.as_bytes()[abs - 1].is_ascii_alphanumeric();
            let after_start = abs + prefix.len();
            if before_ok {
                let rest = lower[after_start..].trim_start();
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    return Some(digits);
                }
            }
            idx = abs + 1;
        }
    }
    None
}

fn base_title(title: &str) -> &str {
    match title.rfind(" #") {
        Some(idx) if idx > 0 => &title[..idx],
        _ => title,
    }
}

/// Replaces Venom wireless BayOutput / BayNorm links with direct cables.
pub fn bypass_wireless_links(patch: &PatchDocument) -> PatchDocument {
    let mut by_id: HashMap<i64, &PatchModule> = HashMap::new();
    for m in &patch.modules {
        by_id.entry(m.id).or_insert(m);
    }
    let is_bay_input = |id: i64| {
        by_id
            .get(&id)
            .is_some_and(|m| m.plugin == "Venom" && m.model == "BayInput")
    };
    let is_norm = |id: i64| {
        by_id
            .get(&id)
            .is_some_and(|m| m.plugin == "Venom" && m.model == "BayNorm")
    };
    let source_of = |id: i64| -> Option<i64> {
        let m = by_id.get(&id)?;
        if m.plugin == "Venom"
            && matches!(m.model.as_str(), "BayOutput" | "BayNorm")
            && let Some(src) = m.linked_source_id
            && is_bay_input(src)
        {
            Some(src)
        } else {
            None
        }
    };

    let linked_inputs: HashSet<i64> = patch
        .modules
        .iter()
        .filter_map(|m| source_of(m.id))
        .collect();
    if linked_inputs.is_empty() {
        return patch.clone();
    }

    let mut feeding: HashMap<PortRef, &PatchCable> = HashMap::new();
    for c in &patch.cables {
        feeding.entry(c.to).or_insert(c);
    }

    fn driver(
        output: PortRef,
        seen: &mut HashSet<i64>,
        source_of: &dyn Fn(i64) -> Option<i64>,
        is_norm: &dyn Fn(i64) -> bool,
        feeding: &HashMap<PortRef, &PatchCable>,
    ) -> Option<PortRef> {
        let Some(source) = source_of(output.module_id) else {
            return Some(output);
        };
        if !seen.insert(output.module_id) {
            return None;
        }
        let upstream = feeding
            .get(&PortRef {
                module_id: source,
                port_id: output.port_id,
            })
            .copied()
            .or_else(|| {
                if is_norm(output.module_id) {
                    feeding
                        .get(&PortRef {
                            module_id: output.module_id,
                            port_id: output.port_id,
                        })
                        .copied()
                } else {
                    None
                }
            });
        upstream.and_then(|c| driver(c.from, seen, source_of, is_norm, feeding))
    }

    let mut cables = Vec::new();
    for c in &patch.cables {
        if linked_inputs.contains(&c.to.module_id)
            || (is_norm(c.to.module_id) && source_of(c.to.module_id).is_some())
        {
            continue;
        }
        if source_of(c.from.module_id).is_none() {
            cables.push(c.clone());
            continue;
        }
        let mut seen = HashSet::new();
        if let Some(drv) = driver(c.from, &mut seen, &source_of, &is_norm, &feeding) {
            let mut rewritten = c.clone();
            rewritten.from = drv;
            cables.push(rewritten);
        }
    }

    PatchDocument {
        version: patch.version.clone(),
        modules: patch.modules.clone(),
        cables,
        is_strip: patch.is_strip,
    }
}

fn expander_base_of(modules: &[PatchModule], catalog: &VcvCatalog) -> HashMap<i64, i64> {
    let by_id: HashMap<i64, &PatchModule> = modules.iter().map(|m| (m.id, m)).collect();
    let is_expander = |m: &PatchModule| -> bool {
        let tagged = catalog
            .module_info(&m.plugin, &m.model)
            .is_some_and(|i| i.tags.iter().any(|t| t.eq_ignore_ascii_case("Expander")));
        tagged || m.model.to_ascii_lowercase().contains("expander")
    };

    let walk = |start: &PatchModule, go_left: bool| -> Option<i64> {
        let mut seen = HashSet::new();
        seen.insert(start.id);
        let mut cur = start;
        while let Some(next_id) = if go_left {
            cur.left_module_id
        } else {
            cur.right_module_id
        } {
            let neighbour = by_id.get(&next_id)?;
            if neighbour.plugin != start.plugin || !seen.insert(next_id) {
                return None;
            }
            if !is_expander(neighbour) {
                return Some(neighbour.id);
            }
            cur = neighbour;
        }
        None
    };

    let mut base_of = HashMap::new();
    for m in modules {
        if is_expander(m)
            && let Some(base) = walk(m, true).or_else(|| walk(m, false))
        {
            base_of.insert(m.id, base);
        }
    }
    base_of
}

fn expander_name(expander: &PatchModule, catalog: &VcvCatalog) -> String {
    let raw = catalog
        .module_info(&expander.plugin, &expander.model)
        .and_then(|i| i.name.as_deref())
        .unwrap_or(&expander.model);
    let trimmed = remove_ci_word(raw, "Expander");
    if trimmed.is_empty() {
        raw.to_string()
    } else {
        trimmed
    }
}

fn remove_ci_word(text: &str, needle: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let lower_n = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&lower_n) {
        out.push_str(&text[i..i + pos]);
        i += pos + needle.len();
    }
    out.push_str(&text[i..]);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn build_titles(modules: &[PatchModule], catalog: &VcvCatalog) -> HashMap<i64, String> {
    let mut grouped: BTreeMap<String, Vec<&PatchModule>> = BTreeMap::new();
    for m in modules {
        let base_name = catalog
            .module_info(&m.plugin, &m.model)
            .and_then(|i| i.name.as_deref())
            .unwrap_or(&m.model)
            .to_string();
        grouped.entry(base_name).or_default().push(m);
    }
    let mut titles = HashMap::new();
    for (name, mut mods) in grouped {
        mods.sort_by_key(|m| (m.row, m.column, m.id));
        let count = mods.len();
        for (i, m) in mods.into_iter().enumerate() {
            if count == 1 {
                titles.insert(m.id, name.clone());
            } else {
                titles.insert(m.id, format!("{name} #{}", i + 1));
            }
        }
    }
    titles
}

fn type_cables(
    patch: &PatchDocument,
    by_id: &HashMap<i64, &PatchModule>,
    base_of: &HashMap<i64, i64>,
    titles: &HashMap<i64, String>,
    role_by_id: &HashMap<i64, Role>,
    catalog: &VcvCatalog,
    diagnostics: &mut Vec<String>,
) -> Vec<AnalyzedCable> {
    let mut cables = Vec::new();
    let mut labeled_inputs = Vec::new();
    let mut plugged_counts: HashMap<PortRef, Vec<usize>> = HashMap::new();

    for original in &patch.cables {
        let (Some(from_mod), Some(to_mod)) = (
            by_id.get(&original.from.module_id),
            by_id.get(&original.to.module_id),
        ) else {
            continue;
        };

        let raw_out = catalog.output_name(&from_mod.plugin, &from_mod.model, original.from.port_id);
        let raw_in = catalog.input_name(&to_mod.plugin, &to_mod.model, original.to.port_id);

        let port_on = |m: &PatchModule, p: String| -> String {
            if base_of.contains_key(&m.id) {
                format!("{} › {p}", expander_name(m, catalog))
            } else {
                p
            }
        };

        let out_labeled =
            resolve_port_label(from_mod, original.from.port_id, raw_out.clone(), false);
        let in_labeled = resolve_port_label(to_mod, original.to.port_id, raw_in.clone(), true);

        let from_port = port_on(
            from_mod,
            resolve_midi_learn_name(from_mod, original.from.port_id, out_labeled.clone(), true),
        );
        let mut to_port = port_on(
            to_mod,
            resolve_midi_learn_name(to_mod, original.to.port_id, in_labeled, true),
        );
        let labeled_to = is_port_labeled(to_mod, original.to.port_id, true);
        let typed_from_port = port_on(
            from_mod,
            resolve_midi_learn_name(from_mod, original.from.port_id, out_labeled, false),
        );
        let typed_to_port = to_port.clone();

        let from_id = *base_of
            .get(&original.from.module_id)
            .unwrap_or(&original.from.module_id);
        let to_id = *base_of
            .get(&original.to.module_id)
            .unwrap_or(&original.to.module_id);
        let cable = PatchCable {
            id: original.id,
            from: PortRef {
                module_id: from_id,
                port_id: original.from.port_id,
            },
            to: PortRef {
                module_id: to_id,
                port_id: original.to.port_id,
            },
            color: original.color.clone(),
        };

        let to_base = &by_id[&to_id];
        let mut routes = Vec::new();
        if !base_of.contains_key(&to_mod.id) {
            routes = mod_routes(to_base, cable.to.port_id, catalog);
            if let Some(matrix) = catalog
                .module_info(&to_base.plugin, &to_base.model)
                .and_then(|i| i.modulation.as_ref())
                && cable.to.port_id >= matrix.first_input
                && cable.to.port_id < matrix.first_input + matrix.input_count
            {
                if routes.is_empty() {
                    diagnostics.push(format!(
                        "{} → {} '{}': no target knob has a depth for this input",
                        titles[&from_id], titles[&to_id], to_port
                    ));
                } else {
                    let route_desc = routes
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    to_port = format!(
                        "Mod {} → {}",
                        cable.to.port_id - matrix.first_input + 1,
                        route_desc
                    );
                }
            }
        }

        let to_takes = if base_of.contains_key(&to_mod.id) {
            None
        } else {
            catalog
                .module_info(&to_base.plugin, &to_base.model)
                .and_then(|i| signal_inputs_of(&i.tags))
        };

        let (signal, mismatch) = classify_signal(
            cable.color.as_deref(),
            &typed_from_port,
            &typed_to_port,
            role_by_id[&from_id],
            role_by_id[&to_id],
            to_takes,
        );
        if let Some(m) = mismatch {
            diagnostics.push(format!(
                "{} '{}' → {} '{}': {m}",
                titles[&from_id], from_port, titles[&to_id], to_port
            ));
        }

        let intent = if !routes.is_empty() {
            suggest_mod_intent(&titles[&from_id], role_by_id[&from_id], &routes)
        } else {
            suggest_intent(
                signal,
                &titles[&from_id],
                from_port.rsplit(" › ").next().unwrap_or(&from_port),
                &titles[&to_id],
                to_port.rsplit(" › ").next().unwrap_or(&to_port),
                role_by_id[&from_id],
                role_by_id[&to_id],
                labeled_to,
            )
        };

        let idx = cables.len();
        cables.push(AnalyzedCable {
            cable,
            from_port,
            to_port,
            signal,
            suggested_intent: intent,
            routes,
        });
        labeled_inputs.push(labeled_to);
        plugged_counts.entry(original.to).or_default().push(idx);
    }

    // Pass-through signal propagation along pitch/router chains.
    let own: Vec<SignalType> = cables.iter().map(|c| c.signal).collect();
    let passing: HashSet<i64> = role_by_id
        .iter()
        .filter_map(|(&id, &role)| matches!(role, Role::Pitch | Role::Router).then_some(id))
        .collect();
    let max_rounds = passing.len() + 1;
    for _ in 0..max_rounds {
        let mut fed_into: HashMap<i64, Vec<&AnalyzedCable>> = HashMap::new();
        for c in &cables {
            fed_into.entry(c.cable.to.module_id).or_default().push(c);
        }
        let passed_on: HashMap<i64, Option<SignalType>> = passing
            .iter()
            .map(|&id| {
                let empty = Vec::new();
                let fed = fed_into.get(&id).unwrap_or(&empty);
                (id, passed_signal(role_by_id[&id], fed))
            })
            .collect();

        let mut changed = 0usize;
        for i in 0..cables.len() {
            let from = cables[i].cable.from.module_id;
            if !passing.contains(&from)
                || knows_palette_color(cables[i].cable.color.as_deref())
                || (role_by_id[&from] == Role::Router
                    && cables[i].routes.is_empty()
                    && signal_from_port_name(&cables[i].to_port) == Some(SignalType::Pitch))
            {
                continue;
            }
            let wanted = passed_on.get(&from).and_then(|v| *v).unwrap_or(own[i]);
            if wanted != cables[i].signal {
                cables[i].signal = wanted;
                if wanted != own[i] {
                    let from_port_short = cables[i]
                        .from_port
                        .rsplit(" › ")
                        .next()
                        .unwrap_or(&cables[i].from_port);
                    let to_port_short = cables[i]
                        .to_port
                        .rsplit(" › ")
                        .next()
                        .unwrap_or(&cables[i].to_port);
                    cables[i].suggested_intent = suggest_intent(
                        wanted,
                        &titles[&from],
                        from_port_short,
                        &titles[&cables[i].cable.to.module_id],
                        to_port_short,
                        role_by_id[&from],
                        role_by_id[&cables[i].cable.to.module_id],
                        labeled_inputs[i],
                    );
                }
                changed += 1;
            }
        }
        if changed == 0 {
            break;
        }
    }

    for (_, indices) in plugged_counts {
        if indices.len() > 1 {
            let first = &cables[indices[0]];
            diagnostics.push(format!(
                "{} '{}' has {} stacked cables",
                titles[&first.cable.to.module_id],
                first.to_port,
                indices.len()
            ));
        }
    }

    cables
}

fn mod_routes(module: &PatchModule, input_id: i32, catalog: &VcvCatalog) -> Vec<ModRoute> {
    let Some(matrix) = catalog
        .module_info(&module.plugin, &module.model)
        .and_then(|i| i.modulation.as_ref())
    else {
        return Vec::new();
    };
    let modulator = input_id - matrix.first_input;
    if modulator < 0 || modulator >= matrix.input_count {
        return Vec::new();
    }
    let mut routes = Vec::new();
    for (target_idx, target) in matrix.targets.iter().enumerate() {
        let param_id =
            matrix.first_depth_param + (target_idx as i32 * matrix.input_count) + modulator;
        let depth = module.params.get(&param_id).copied().unwrap_or(0.0);
        if depth.abs() >= 0.005 {
            routes.push(ModRoute {
                target: target.clone(),
                depth,
            });
        }
    }
    routes
}

fn passed_signal(role: Role, fed: &[&AnalyzedCable]) -> Option<SignalType> {
    if role == Role::Pitch {
        let sigs: HashSet<SignalType> = fed
            .iter()
            .filter(|c| !is_control_port(&c.to_port))
            .map(|c| c.signal)
            .collect();
        return if sigs.len() == 1 {
            sigs.into_iter().next()
        } else {
            None
        };
    }
    if role == Role::Router {
        let sigs: HashSet<SignalType> = fed
            .iter()
            .filter(|c| !is_control_port(&c.to_port))
            .map(|c| c.signal)
            .collect();
        return if sigs.len() == 1 {
            sigs.into_iter().next()
        } else {
            None
        };
    }
    None
}

fn resolve_contextual_roles(
    shown: &[PatchModule],
    cables: &[AnalyzedCable],
    role_by_id: &mut HashMap<i64, Role>,
) {
    let has_outputs = |id: i64| cables.iter().any(|c| c.cable.from.module_id == id);

    for m in shown {
        if role_by_id[&m.id] == Role::Router {
            let mut counts: BTreeMap<SignalType, usize> = BTreeMap::new();
            for c in cables {
                if c.cable.to.module_id == m.id && !is_control_port(&c.to_port) {
                    *counts.entry(c.signal).or_default() += 1;
                }
            }
            let routed = counts
                .into_iter()
                .max_by_key(|&(sig, count)| (count, std::cmp::Reverse(sig)))
                .map(|(sig, _)| sig);
            let resolved = match routed {
                Some(SignalType::Audio) => Role::Modifier,
                Some(SignalType::Pitch) => Role::Pitch,
                Some(SignalType::Gate) => Role::Time,
                _ => Role::Controller,
            };
            role_by_id.insert(m.id, resolved);
        }
    }

    for m in shown {
        if role_by_id[&m.id] == Role::Controller
            && cables
                .iter()
                .any(|c| c.cable.to.module_id == m.id && c.signal == SignalType::Audio)
            && cables
                .iter()
                .any(|c| c.cable.from.module_id == m.id && c.signal == SignalType::Audio)
        {
            role_by_id.insert(m.id, Role::Modifier);
        }
    }

    for m in shown {
        if role_by_id[&m.id] == Role::Monitor && has_outputs(m.id) {
            role_by_id.insert(m.id, Role::Controller);
        }
    }
    for m in shown {
        if role_by_id[&m.id] == Role::Performance && !has_outputs(m.id) {
            role_by_id.insert(m.id, Role::Monitor);
        }
    }

    let is_outer = |id: i64, roles: &HashMap<i64, Role>| {
        matches!(roles[&id], Role::Io | Role::Performance) && has_outputs(id)
    };
    let mut relays = Vec::new();
    let mut outside = HashSet::new();
    for m in shown {
        if !is_outer(m.id, role_by_id) {
            continue;
        }
        if cables.iter().any(|c| c.cable.to.module_id == m.id) {
            relays.push(m.id);
        } else {
            outside.insert(m.id);
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for &relay in &relays {
            if outside.contains(&relay) {
                continue;
            }
            if cables
                .iter()
                .filter(|c| c.cable.to.module_id == relay)
                .all(|c| outside.contains(&c.cable.from.module_id))
            {
                outside.insert(relay);
                changed = true;
            }
        }
    }

    for relay in relays {
        if outside.contains(&relay) {
            continue;
        }
        let mut counts: BTreeMap<SignalType, usize> = BTreeMap::new();
        for c in cables {
            if c.cable.from.module_id == relay {
                *counts.entry(c.signal).or_default() += 1;
            }
        }
        if let Some((dominant, _)) = counts
            .into_iter()
            .max_by_key(|&(sig, count)| (count, std::cmp::Reverse(sig)))
        {
            let resolved = match dominant {
                SignalType::Pitch => Role::Pitch,
                SignalType::Gate => Role::Time,
                _ => Role::Controller,
            };
            role_by_id.insert(relay, resolved);
        }
    }
}

fn band_for(module_id: i64, role: Role, cables: &[AnalyzedCable]) -> Band {
    match role {
        Role::Time => Band::Time,
        Role::Pitch => Band::Pitch,
        Role::Effect | Role::Modifier => {
            let has_audio = cables.iter().any(|c| {
                c.signal == SignalType::Audio
                    && (c.cable.from.module_id == module_id || c.cable.to.module_id == module_id)
            });
            if has_audio {
                Band::Bus
            } else {
                Band::Modulation
            }
        }
        Role::Mixer => Band::Bus,
        Role::Io => {
            if cables.iter().any(|c| c.cable.from.module_id == module_id) {
                Band::External
            } else {
                Band::Bus
            }
        }
        Role::Performance => Band::External,
        Role::Monitor => Band::Monitor,
        Role::Controller | Role::Source | Role::Router => Band::Modulation,
    }
}

fn pitch_origin<'a>(
    source_id: i64,
    cables: &'a [AnalyzedCable],
    role_by_id: &HashMap<i64, Role>,
) -> Option<&'a AnalyzedCable> {
    let mut current = source_id;
    let mut seen = HashSet::new();
    seen.insert(current);
    let mut origin = None;
    loop {
        let Some(pitch_in) = cables
            .iter()
            .find(|c| c.cable.to.module_id == current && c.signal == SignalType::Pitch)
        else {
            return origin;
        };
        if !seen.insert(pitch_in.cable.from.module_id) {
            return origin;
        }
        current = pitch_in.cable.from.module_id;
        origin = Some(pitch_in);
        if role_by_id[&current] != Role::Pitch {
            return origin;
        }
    }
}

struct AudioTopology {
    insert_host: HashMap<i64, i64>,
    voices: Vec<Vec<i64>>,
}

fn build_audio_topology(
    rack_order: &[i64],
    cables: &[AnalyzedCable],
    role_by_id: &HashMap<i64, Role>,
    origin_of: &HashMap<i64, Option<AnalyzedCable>>,
) -> AudioTopology {
    let audio: Vec<&AnalyzedCable> = cables
        .iter()
        .filter(|c| c.signal == SignalType::Audio && c.cable.from.module_id != c.cable.to.module_id)
        .collect();
    let insert_host = find_inserts(&audio, role_by_id);

    let is_processor = |id: i64| {
        matches!(
            role_by_id[&id],
            Role::Source | Role::Modifier | Role::Effect
        ) && !insert_host.contains_key(&id)
    };
    let mut parent: HashMap<i64, i64> = rack_order
        .iter()
        .copied()
        .filter(|&id| is_processor(id))
        .map(|id| (id, id))
        .collect();

    fn find_root(parent: &mut HashMap<i64, i64>, id: i64) -> i64 {
        let p = parent[&id];
        if p == id {
            id
        } else {
            let root = find_root(parent, p);
            parent.insert(id, root);
            root
        }
    }

    let union_all = |parent: &mut HashMap<i64, i64>| {
        for c in &audio {
            let from = c.cable.from.module_id;
            let to = c.cable.to.module_id;
            if parent.contains_key(&from) && parent.contains_key(&to) {
                let rf = find_root(parent, from);
                let rt = find_root(parent, to);
                if rf != rt {
                    parent.insert(rf, rt);
                }
            }
        }
    };
    union_all(&mut parent);

    let origins_of = |parent: &mut HashMap<i64, i64>, root: i64| -> Vec<Option<PortRef>> {
        let ids: Vec<i64> = parent.keys().copied().collect();
        let mut set = Vec::new();
        for id in ids {
            if find_root(parent, id) == root && role_by_id[&id] == Role::Source {
                let o = origin_of
                    .get(&id)
                    .and_then(|c| c.as_ref().map(|c| c.cable.from));
                if !set.contains(&o) {
                    set.push(o);
                }
            }
        }
        set
    };

    let mut changed = true;
    while changed {
        changed = false;
        let mixers: Vec<i64> = rack_order
            .iter()
            .copied()
            .filter(|id| {
                role_by_id[id] == Role::Mixer
                    && !insert_host.contains_key(id)
                    && !parent.contains_key(id)
            })
            .collect();
        for mixer in mixers {
            let mut feeders = Vec::new();
            for c in &audio {
                if c.cable.to.module_id == mixer && !feeders.contains(&c.cable.from.module_id) {
                    feeders.push(c.cable.from.module_id);
                }
            }
            if feeders.is_empty() || !feeders.iter().all(|f| parent.contains_key(f)) {
                continue;
            }
            let mut roots = Vec::new();
            for &f in &feeders {
                let r = find_root(&mut parent, f);
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
            let per_voice: Vec<Vec<Option<PortRef>>> =
                roots.iter().map(|&r| origins_of(&mut parent, r)).collect();
            let mut all_origins: Vec<Option<PortRef>> = Vec::new();
            for list in &per_voice {
                for &o in list {
                    if !all_origins.contains(&o) {
                        all_origins.push(o);
                    }
                }
            }
            let mut targets = Vec::new();
            for c in &audio {
                if c.cable.from.module_id == mixer && !targets.contains(&c.cable.to.module_id) {
                    targets.push(c.cable.to.module_id);
                }
            }
            let feeder0_root = find_root(&mut parent, feeders[0]);
            let submix = per_voice.len() == 1
                && !targets.is_empty()
                && targets
                    .iter()
                    .all(|t| parent.contains_key(t) && find_root(&mut parent, *t) == feeder0_root);
            let same_notes = per_voice.len() >= 2
                && per_voice.iter().all(|o| !o.is_empty())
                && all_origins.len() == 1
                && all_origins[0].is_some();
            if submix || same_notes {
                parent.insert(mixer, mixer);
                union_all(&mut parent);
                changed = true;
            }
        }
    }

    let mut audio_touched = HashSet::new();
    for c in &audio {
        audio_touched.insert(c.cable.from.module_id);
        audio_touched.insert(c.cable.to.module_id);
    }

    let order_idx: HashMap<i64, usize> = rack_order
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    let mut comp_map: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let all_parent_keys: Vec<i64> = parent.keys().copied().collect();
    for id in all_parent_keys {
        if audio_touched.contains(&id) {
            let root = find_root(&mut parent, id);
            comp_map.entry(root).or_default().push(id);
        }
    }

    let mut components: Vec<Vec<i64>> = comp_map
        .into_values()
        .filter(|g| g.iter().any(|id| role_by_id[id] == Role::Source))
        .collect();
    components.sort_by_key(|g| {
        g.iter()
            .filter_map(|id| order_idx.get(id).copied())
            .min()
            .unwrap_or(usize::MAX)
    });

    let mut voices = Vec::with_capacity(components.len());
    for component in components {
        let mut set: HashSet<i64> = component.iter().copied().collect();
        peel_output_effects(&mut set, &audio, role_by_id);
        let mut nodes: Vec<i64> = component
            .into_iter()
            .filter(|id| set.contains(id))
            .collect();
        nodes.sort_by_key(|id| order_idx.get(id).copied().unwrap_or(usize::MAX));
        let mut edges = Vec::new();
        for c in &audio {
            let e = (c.cable.from.module_id, c.cable.to.module_id);
            if set.contains(&e.0) && set.contains(&e.1) && !edges.contains(&e) {
                edges.push(e);
            }
        }
        voices.push(graph_topological(&nodes, &edges));
    }

    AudioTopology {
        insert_host,
        voices,
    }
}

fn peel_output_effects(
    voice: &mut HashSet<i64>,
    audio: &[&AnalyzedCable],
    role_by_id: &HashMap<i64, Role>,
) {
    let mut peeled = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        let candidates: Vec<i64> = voice
            .iter()
            .copied()
            .filter(|id| role_by_id[id] == Role::Effect)
            .collect();
        for id in candidates {
            let mut targets = Vec::new();
            for c in audio {
                if c.cable.from.module_id == id && !targets.contains(&c.cable.to.module_id) {
                    targets.push(c.cable.to.module_id);
                }
            }
            if !targets.is_empty()
                && targets.iter().all(|t| {
                    peeled.contains(t) || (!voice.contains(t) && role_by_id[t] == Role::Io)
                })
            {
                voice.remove(&id);
                peeled.insert(id);
                changed = true;
            }
        }
    }
}

fn find_inserts(audio: &[&AnalyzedCable], role_by_id: &HashMap<i64, Role>) -> HashMap<i64, i64> {
    let mut next: HashMap<i64, Vec<i64>> = HashMap::new();
    for c in audio {
        let list = next.entry(c.cable.from.module_id).or_default();
        if !list.contains(&c.cable.to.module_id) {
            list.push(c.cable.to.module_id);
        }
    }
    let mut starts: Vec<&AnalyzedCable> = audio.to_vec();
    starts.sort_by_key(|c| {
        if c.from_port.to_ascii_lowercase().contains("send") {
            0
        } else {
            1
        }
    });

    let mut insert_host = HashMap::new();
    for start in starts {
        let host = start.cable.from.module_id;
        let first = start.cable.to.module_id;
        if !matches!(role_by_id[&first], Role::Modifier | Role::Effect)
            || insert_host.contains_key(&first)
            || insert_host.contains_key(&host)
            || insert_host.values().any(|&v| v == first)
        {
            continue;
        }
        let chain = chain_back_to(host, first, &next, role_by_id);
        let closed = !chain.is_empty()
            && audio
                .iter()
                .filter(|c| chain.contains(&c.cable.to.module_id))
                .all(|c| c.cable.from.module_id == host || chain.contains(&c.cable.from.module_id));
        let returns_audio = role_by_id[&host] != Role::Source
            && audio
                .iter()
                .filter(|c| chain.contains(&c.cable.from.module_id) && c.cable.to.module_id == host)
                .all(|c| !is_modulation_input(&c.to_port));
        if closed && returns_audio {
            for member in chain {
                insert_host.entry(member).or_insert(host);
            }
        }
    }
    insert_host
}

fn chain_back_to(
    host: i64,
    first: i64,
    next: &HashMap<i64, Vec<i64>>,
    role_by_id: &HashMap<i64, Role>,
) -> HashSet<i64> {
    let mut reachable = HashSet::new();
    let mut stack = vec![first];
    while let Some(id) = stack.pop() {
        if id == host || matches!(role_by_id[&id], Role::Mixer | Role::Io) || !reachable.insert(id)
        {
            continue;
        }
        if let Some(targets) = next.get(&id) {
            for &t in targets {
                stack.push(t);
            }
        }
    }

    let mut back_to_host = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for &id in &reachable {
            if back_to_host.contains(&id) {
                continue;
            }
            if let Some(targets) = next.get(&id)
                && (targets.contains(&host) || targets.iter().any(|t| back_to_host.contains(t)))
            {
                back_to_host.insert(id);
                changed = true;
            }
        }
    }
    back_to_host
}

// =========================================================================
// Signal classification & intent suggestion
// =========================================================================

pub fn palette_color_signal(color: Option<&str>) -> Option<SignalType> {
    let hex = color?.trim();
    if hex.eq_ignore_ascii_case("#ff5252") {
        Some(SignalType::Audio)
    } else if hex.eq_ignore_ascii_case("#ffd452") {
        Some(SignalType::Pitch)
    } else if hex.eq_ignore_ascii_case("#52ff7d") || hex.eq_ignore_ascii_case("#a8ff52") {
        Some(SignalType::Cv)
    } else if hex.eq_ignore_ascii_case("#52beff") {
        Some(SignalType::Gate)
    } else {
        None
    }
}

pub fn knows_palette_color(color: Option<&str>) -> bool {
    palette_color_signal(color).is_some()
}

pub fn signal_from_port_name(port_name: &str) -> Option<SignalType> {
    const PITCH_WORDS: &[&str] = &["v/oct", "1v/octave", "pitch", "v/o", "voct"];
    const GATE_WORDS: &[&str] = &[
        "gate", "trig", "retrig", "clock", "clk", "reset", "run", "start", "stop", "continue",
    ];
    let lower = port_name.to_ascii_lowercase();
    if PITCH_WORDS.iter().any(|w| lower.contains(w)) {
        return Some(SignalType::Pitch);
    }
    if GATE_WORDS.iter().any(|w| contains_word_start(&lower, w)) {
        return Some(SignalType::Gate);
    }
    None
}

pub fn signal_inputs_of(tags: &[String]) -> Option<SignalType> {
    const GATE_LOGIC_TAGS: &[&str] = &[
        "Logic",
        "Clock generator",
        "Clock modulator",
        "Sequencer",
        "Utility",
        "Polyphonic",
        "Dual",
        "Quad",
        "Hardware clone",
        "Visual",
        "Expander",
    ];
    let canonical = canonical_tags(tags);
    let has = |t: &str| canonical.iter().any(|c| c.eq_ignore_ascii_case(t));
    if has("Logic")
        && canonical
            .iter()
            .all(|t| GATE_LOGIC_TAGS.iter().any(|g| g.eq_ignore_ascii_case(t)))
    {
        return Some(SignalType::Gate);
    }
    if has("Envelope follower") {
        Some(SignalType::Audio)
    } else if has("Sample and hold") {
        Some(SignalType::Cv)
    } else {
        None
    }
}

pub fn is_control_port(port_name: &str) -> bool {
    const CONTROL_WORDS: &[&str] = &["cv", "select", "index", "step", "steps"];
    signal_from_port_name(port_name) == Some(SignalType::Gate)
        || has_any_word(port_name, CONTROL_WORDS)
}

pub fn is_modulation_input(port_name: &str) -> bool {
    const MOD_WORDS: &[&str] = &[
        "cv",
        "fm",
        "pm",
        "am",
        "sync",
        "mod",
        "modulation",
        "phase",
        "exp",
        "lin",
        "rate",
        "freq",
        "frequency",
        "cutoff",
        "fold",
        "shape",
        "pwm",
        "pw",
    ];
    signal_from_port_name(port_name).is_some() || has_any_word(port_name, MOD_WORDS)
}

pub fn classify_signal(
    color: Option<&str>,
    from_port: &str,
    to_port: &str,
    from_role: Role,
    to_role: Role,
    to_takes: Option<SignalType>,
) -> (SignalType, Option<String>) {
    const CV_WORDS: &[&str] = &[
        "env",
        "envelope",
        "lfo",
        "cv",
        "phase",
        "mod",
        "modulation",
        "s&h",
        "sample",
        "random",
        "level",
    ];
    const EXPRESSION_WORDS: &[&str] = &[
        "velocity",
        "vel",
        "aftertouch",
        "aft",
        "pressure",
        "expression",
        "mod wheel",
        "modwheel",
    ];
    const COMMAND_INPUT_WORDS: &[&str] = &["cv", "fm", "pm", "sync", "modulation"];
    const CV_INPUT_WORDS: &[&str] = &["cv"];

    let by_color = palette_color_signal(color);
    let by_from = signal_from_port_name(from_port);
    let by_to = signal_from_port_name(to_port);

    let source_audio = from_role == Role::Source
        && matches!(to_role, Role::Modifier | Role::Effect | Role::Mixer)
        && by_from.is_none()
        && !has_any_word(from_port, CV_WORDS)
        && by_to.is_none()
        && !has_any_word(to_port, COMMAND_INPUT_WORDS);
    if source_audio
        && let Some(wrong) = by_color
        && wrong != SignalType::Audio
    {
        return (
            SignalType::Audio,
            Some(format!(
                "color says {}, but a source's audio output into a {} is audio",
                wrong.short_label(),
                to_role.label()
            )),
        );
    }

    if let Some(color_sig) = by_color {
        let mismatch = by_from.filter(|&p| p != color_sig).map(|p| {
            format!(
                "color says {}, output '{}' says {}",
                color_sig.short_label(),
                from_port,
                p.short_label()
            )
        });
        return (color_sig, mismatch);
    }

    if let Some(sig) = by_from {
        return (sig, None);
    }
    if has_any_word(from_port, EXPRESSION_WORDS) {
        return (SignalType::Cv, None);
    }
    if let Some(sig) = by_to {
        return (sig, None);
    }
    if let Some(taken) = to_takes
        && !has_any_word(to_port, CV_INPUT_WORDS)
    {
        return (taken, None);
    }

    let audio_from = matches!(
        from_role,
        Role::Source | Role::Modifier | Role::Effect | Role::Mixer
    );
    let audio_to = (matches!(
        to_role,
        Role::Modifier | Role::Effect | Role::Mixer | Role::Io | Role::Router
    ) || matches!(from_role, Role::Source | Role::Effect))
        && !has_any_word(to_port, CV_INPUT_WORDS);
    (
        if audio_from && audio_to {
            SignalType::Audio
        } else {
            SignalType::Cv
        },
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn suggest_intent(
    signal: SignalType,
    from_title: &str,
    _from_port: &str,
    to_title: &str,
    to_port: &str,
    from_role: Role,
    to_role: Role,
    labeled_port: bool,
) -> String {
    let port = to_port.to_ascii_lowercase();
    let shown = if labeled_port { to_port } else { port.as_str() };
    let short_from = from_title.split(' ').next().unwrap_or(from_title);
    match signal {
        SignalType::Audio => {
            if port.contains("channel") || matches!(to_role, Role::Mixer | Role::Effect) {
                format!("into {to_title}")
            } else {
                "audio".to_string()
            }
        }
        SignalType::Pitch => {
            if port.contains("pitch") || port.contains("v/oct") {
                "plays the notes".to_string()
            } else {
                format!("pitch tracks {shown}")
            }
        }
        SignalType::Gate => {
            if to_role == Role::Router
                && (port.contains("trig") || port.contains("clock") || port.contains("clk"))
            {
                "steps to the next channel".to_string()
            } else if port.contains("retrig") {
                "retriggers on each note".to_string()
            } else if port.contains("gate") || port.contains("trig") {
                if to_role == Role::Controller {
                    "fires the envelope / new value".to_string()
                } else {
                    "triggers".to_string()
                }
            } else if port.contains("clock") || port.contains("clk") {
                "sets the tempo".to_string()
            } else if port.contains("reset") {
                "restarts".to_string()
            } else if port.contains("run") {
                "starts / stops".to_string()
            } else {
                format!("triggers {shown}")
            }
        }
        SignalType::Cv => {
            if matches!(from_role, Role::Io | Role::Performance) {
                format!("hand control of {shown}")
            } else {
                format!("{short_from} moves {shown}")
            }
        }
    }
}

fn suggest_mod_intent(from_title: &str, from_role: Role, routes: &[ModRoute]) -> String {
    let mut sorted: Vec<&ModRoute> = routes.iter().collect();
    sorted.sort_by(|a, b| {
        b.depth
            .abs()
            .partial_cmp(&a.depth.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let targets = sorted
        .into_iter()
        .map(|r| r.target.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" & ");
    if matches!(from_role, Role::Io | Role::Performance) {
        format!("hand control of {targets}")
    } else {
        let short_from = from_title.split(' ').next().unwrap_or(from_title);
        format!("{short_from} moves {targets}")
    }
}

fn has_any_word(text: &str, words: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    words.iter().any(|word| {
        let wb = word.as_bytes();
        let mut start = 0;
        while let Some(pos) = lower[start..].find(word) {
            let abs = start + pos;
            let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_lowercase();
            let after = abs + wb.len();
            let after_ok = after == bytes.len() || !bytes[after].is_ascii_lowercase();
            if before_ok && after_ok {
                return true;
            }
            start = abs + 1;
        }
        false
    })
}

fn contains_word_start(lower: &str, word: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut start = 0;
    while let Some(pos) = lower[start..].find(word) {
        let abs = start + pos;
        if abs == 0 || !bytes[abs - 1].is_ascii_alphabetic() {
            return true;
        }
        start = abs + 1;
    }
    false
}

// =========================================================================
// GraphOrder (Topological, Loops, Depths)
// =========================================================================

pub fn graph_topological(nodes: &[i64], edges: &[(i64, i64)]) -> Vec<i64> {
    let count = nodes.len();
    let index_of: HashMap<i64, usize> = nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut outgoing = vec![Vec::<usize>::new(); count];
    let mut incoming = vec![Vec::<usize>::new(); count];
    let mut in_deg = vec![0i32; count];
    let mut out_deg = vec![0i32; count];

    for &(from, to) in edges {
        if from != to
            && let (Some(&f), Some(&t)) = (index_of.get(&from), index_of.get(&to))
        {
            outgoing[f].push(t);
            incoming[t].push(f);
            in_deg[t] += 1;
            out_deg[f] += 1;
        }
    }

    let mut placed = vec![false; count];
    let mut free: BTreeSet<usize> = (0..count).filter(|&i| in_deg[i] == 0).collect();
    let mut result = Vec::with_capacity(count);
    while result.len() < count {
        let next = if let Some(first) = free.pop_first() {
            first
        } else {
            let mut best = None;
            for i in 0..count {
                if !placed[i] {
                    best = match best {
                        None => Some(i),
                        Some(b) if out_deg[i] - in_deg[i] > out_deg[b] - in_deg[b] => Some(i),
                        _ => best,
                    };
                }
            }
            best.expect("unplaced node")
        };
        placed[next] = true;
        result.push(nodes[next]);
        for &to in &outgoing[next] {
            if !placed[to] {
                in_deg[to] -= 1;
                if in_deg[to] == 0 {
                    free.insert(to);
                }
            }
        }
        for &from in &incoming[next] {
            if !placed[from] {
                out_deg[from] -= 1;
            }
        }
    }
    result
}

pub fn graph_loops(nodes: &[i64], edges: &[(i64, i64)]) -> Vec<Vec<i64>> {
    let input_idx: HashMap<i64, usize> = nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut next: HashMap<i64, Vec<i64>> = nodes.iter().map(|&n| (n, Vec::new())).collect();
    for &(from, to) in edges {
        if from != to
            && let Some(targets) = next.get_mut(&from)
            && !targets.contains(&to)
        {
            targets.push(to);
        }
    }

    struct Tarjan<'a> {
        next: &'a HashMap<i64, Vec<i64>>,
        input_idx: &'a HashMap<i64, usize>,
        index: HashMap<i64, usize>,
        low: HashMap<i64, usize>,
        stack: Vec<i64>,
        on_stack: HashSet<i64>,
        loops: Vec<Vec<i64>>,
    }

    impl Tarjan<'_> {
        fn visit(&mut self, node: i64) {
            let cur_idx = self.index.len();
            self.index.insert(node, cur_idx);
            self.low.insert(node, cur_idx);
            self.stack.push(node);
            self.on_stack.insert(node);

            if let Some(targets) = self.next.get(&node) {
                for &to in targets {
                    if !self.next.contains_key(&to) {
                        continue;
                    }
                    if !self.index.contains_key(&to) {
                        self.visit(to);
                        let low_to = self.low[&to];
                        let low_node = self.low.get_mut(&node).unwrap();
                        *low_node = (*low_node).min(low_to);
                    } else if self.on_stack.contains(&to) {
                        let idx_to = self.index[&to];
                        let low_node = self.low.get_mut(&node).unwrap();
                        *low_node = (*low_node).min(idx_to);
                    }
                }
            }

            if self.low[&node] != self.index[&node] {
                return;
            }
            let mut component = Vec::new();
            while let Some(member) = self.stack.pop() {
                self.on_stack.remove(&member);
                component.push(member);
                if member == node {
                    break;
                }
            }
            if component.len() > 1 {
                component.sort_by_key(|m| self.input_idx[m]);
                self.loops.push(component);
            }
        }
    }

    let mut state = Tarjan {
        next: &next,
        input_idx: &input_idx,
        index: HashMap::new(),
        low: HashMap::new(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        loops: Vec::new(),
    };
    for &node in nodes {
        if !state.index.contains_key(&node) {
            state.visit(node);
        }
    }
    state.loops.sort_by_key(|l| input_idx[&l[0]]);
    state.loops
}

pub fn graph_depths(
    nodes: &[i64],
    edges: &[(i64, i64)],
    weak: &HashSet<(i64, i64)>,
) -> HashMap<i64, usize> {
    let ordering: Vec<(i64, i64)> = if !weak.is_empty() {
        let mut loop_of = HashMap::new();
        for (i, l) in graph_loops(nodes, edges).into_iter().enumerate() {
            for n in l {
                loop_of.insert(n, i);
            }
        }
        edges
            .iter()
            .copied()
            .filter(|e| {
                !(weak.contains(e)
                    && matches!(
                        (loop_of.get(&e.0), loop_of.get(&e.1)),
                        (Some(a), Some(b)) if a == b
                    ))
            })
            .collect()
    } else {
        edges.to_vec()
    };

    let order = graph_topological(nodes, &ordering);
    let position: HashMap<i64, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut depth: HashMap<i64, usize> = nodes.iter().map(|&n| (n, 0)).collect();
    let mut targets: HashMap<i64, Vec<i64>> = HashMap::new();
    for &(from, to) in edges {
        targets.entry(from).or_default().push(to);
    }

    for &node in &order {
        if let Some(list) = targets.get(&node) {
            for &to in list {
                if let Some(&to_pos) = position.get(&to)
                    && to_pos > position[&node]
                {
                    let d = depth[&node] + 1;
                    let entry = depth.entry(to).or_insert(0);
                    *entry = (*entry).max(d);
                }
            }
        }
    }
    depth
}
