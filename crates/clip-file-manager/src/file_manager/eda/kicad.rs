//! KiCad interactive viewer panels for `.kicad_pcb`, `.kicad_sch`, `.kicad_mod`,
//! `.kicad_sym`, `.gbrjob`, and `.kicad_pro`.

use super::ViewerState;
use egui::{Color32, Ui, vec2};
use little_clip_core::eda::kicad::{
    GerberJobDocument, KicadFootprintDocument, KicadPcbDocument, KicadProjectDocument,
    KicadSchematicDocument, KicadSymbolEntry, KicadSymbolLibDocument, PcbLayerBucket,
    project::stackup_layer_rgba,
};

pub(super) fn render_pcb_details(ui: &mut Ui, pcb: &KicadPcbDocument, state: &mut ViewerState) {
    let dims = pcb.outline_mm.unwrap_or([
        pcb.bounds_mm[2] - pcb.bounds_mm[0],
        pcb.bounds_mm[3] - pcb.bounds_mm[1],
    ]);
    ui.strong("KiCad PCB Layout");
    ui.label(format!("Outline: {:.2} × {:.2} mm", dims[0], dims[1]));
    if let Some(t) = pcb.thickness_mm {
        ui.label(format!("Thickness: {t:.2} mm"));
    }
    ui.label(format!(
        "Layers: {} ({} Cu)",
        pcb.layer_count, pcb.copper_layer_count
    ));
    if let Some(count) = pcb.stackup_layer_count {
        ui.label(format!("Stackup: {count} layers"));
    }
    ui.label(format!(
        "Footprints: {} · Nets: {} · Vias: {} · Zones: {}",
        pcb.footprint_count, pcb.net_count, pcb.via_count, pcb.zone_count
    ));
    ui.label(format!(
        "Tracks: {} segs + {} arcs ({:.2} mm)",
        pcb.track_segment_count, pcb.track_arc_count, pcb.track_length_mm
    ));

    ui.separator();
    ui.strong("Layer Visibility");
    for &bucket in &PcbLayerBucket::ALL {
        let mut enabled = state.pcb_layers.contains(&bucket);
        let [r, g, b, _] = bucket.rgba();
        ui.horizontal(|ui| {
            let (swatch, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(swatch, 2.0, Color32::from_rgb(r, g, b));
            if ui.checkbox(&mut enabled, bucket.label()).changed() {
                if enabled {
                    state.pcb_layers.insert(bucket);
                } else {
                    state.pcb_layers.remove(&bucket);
                }
            }
        });
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.filter_query);
    });
    let q = state.filter_query.trim().to_ascii_lowercase();

    ui.strong(format!("Footprints ({})", pcb.footprints.len()));
    for fp in pcb.footprints.iter().take(200) {
        if !q.is_empty()
            && !fp.ref_des.to_ascii_lowercase().contains(&q)
            && !fp.value.to_ascii_lowercase().contains(&q)
            && !fp.lib_id.to_ascii_lowercase().contains(&q)
        {
            continue;
        }
        ui.label(format!(
            "{} · {} ({}) [{} @ {:.1}, {:.1}]",
            fp.ref_des, fp.value, fp.lib_id, fp.layer, fp.pos_mm[0], fp.pos_mm[1]
        ));
    }

    ui.separator();
    ui.strong(format!("Nets ({})", pcb.net_count));
    for net in pcb
        .nets
        .iter()
        .filter(|net| q.is_empty() || net.name.to_ascii_lowercase().contains(&q))
        .take(150)
    {
        if let Some(id) = net.id {
            ui.label(format!("#{id}  {}", net.name));
        } else {
            ui.label(&net.name);
        }
    }
}

pub(super) fn render_schematic_details(
    ui: &mut Ui,
    sch: &KicadSchematicDocument,
    state: &mut ViewerState,
) {
    ui.strong(&sch.sheet_title);
    ui.label(format!("Paper: {}", sch.paper));
    if !sch.revision.is_empty() || !sch.company.is_empty() {
        ui.label(format!("Rev {} · {}", sch.revision, sch.company));
    }
    ui.label(format!(
        "Symbols: {} ({} parts, {} power)",
        sch.symbol_count, sch.component_count, sch.power_symbol_count
    ));
    ui.label(format!(
        "Wires: {} · Buses: {} · Junctions: {} · NC: {}",
        sch.wire_count, sch.bus_count, sch.junction_count, sch.no_connect_count
    ));

    if !sch.sheets.is_empty() {
        ui.separator();
        ui.strong(format!("Subsheets ({})", sch.sheets.len()));
        for sheet in &sch.sheets {
            ui.label(format!("{} ({})", sheet.name, sheet.filename));
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.filter_query);
    });
    let q = state.filter_query.trim().to_ascii_lowercase();

    ui.strong(format!("Net Labels ({})", sch.net_labels.len()));
    for label in sch.net_labels.iter().take(200) {
        if !q.is_empty() && !label.name.to_ascii_lowercase().contains(&q) {
            continue;
        }
        ui.label(format!(
            "[{}] {} ({}×)",
            label.kind.label(),
            label.name,
            label.occurrences
        ));
    }

    ui.separator();
    ui.strong(format!("Symbols ({})", sch.symbols.len()));
    for sym in sch.symbols.iter().filter(|s| !s.is_power).take(200) {
        if !q.is_empty()
            && !sym.reference.to_ascii_lowercase().contains(&q)
            && !sym.value.to_ascii_lowercase().contains(&q)
        {
            continue;
        }
        ui.label(format!(
            "{} · {} ({})",
            sym.reference, sym.value, sym.lib_id
        ));
    }

    if !sch.warnings.is_empty() {
        ui.separator();
        ui.strong("Notes");
        for w in &sch.warnings {
            ui.add(egui::Label::new(w).wrap());
        }
    }
}

pub(super) fn render_footprint_details(ui: &mut Ui, fp: &KicadFootprintDocument) {
    let w = (fp.bounds_mm[2] - fp.bounds_mm[0]).max(0.0);
    let h = (fp.bounds_mm[3] - fp.bounds_mm[1]).max(0.0);
    ui.strong(&fp.name);
    ui.label(format!("Layer: {} · Bounds: {w:.2} × {h:.2} mm", fp.layer));
    if !fp.description.is_empty() {
        ui.add(egui::Label::new(&fp.description).wrap());
    }
    if !fp.attributes.is_empty() {
        ui.label(format!("Attributes: {}", fp.attributes.join(", ")));
    }
    if !fp.models_3d.is_empty() {
        ui.separator();
        ui.strong(format!("3D Models ({})", fp.models_3d.len()));
        for model in &fp.models_3d {
            ui.add(egui::Label::new(model).wrap());
        }
    }
    ui.separator();
    ui.strong(format!("Pads ({})", fp.pads.len()));
    for pad in fp.pads.iter().take(256) {
        let drill = pad
            .drill_mm
            .map_or_else(String::new, |[dx, dy]| format!(" Ø{dx:.2}×{dy:.2}"));
        ui.label(format!(
            "#{} {} {} ({:.2}×{:.2} mm{drill})",
            pad.number, pad.pad_type, pad.shape, pad.size_mm[0], pad.size_mm[1]
        ));
    }
}

pub(super) fn render_symbol_lib_details(
    ui: &mut Ui,
    lib: &KicadSymbolLibDocument,
    state: &mut ViewerState,
) {
    ui.strong(format!(
        "KiCad Symbol Library ({} symbols)",
        lib.symbols.len()
    ));
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.filter_query);
    });
    let q = state.filter_query.trim().to_ascii_lowercase();
    render_symbol_list(ui, &lib.symbols, &q, state);

    if let Some(entry) = lib.symbols.get(state.selected_symbol) {
        render_selected_symbol_info(ui, entry, state);
    }
}

fn render_symbol_list(
    ui: &mut Ui,
    symbols: &[KicadSymbolEntry],
    query: &str,
    state: &mut ViewerState,
) {
    for (idx, sym) in symbols.iter().enumerate() {
        if !query.is_empty() && !sym.name.to_ascii_lowercase().contains(query) {
            continue;
        }
        let selected = state.selected_symbol == idx;
        let suffix = sym
            .extends
            .as_deref()
            .map_or_else(String::new, |p| format!(" -> {p}"));
        let label = format!("{} ({} pins){suffix}", sym.name, sym.pins.len());
        if ui.selectable_label(selected, label).clicked() && !selected {
            state.selected_symbol = idx;
            state.selected_unit = sym.units[0];
        }
    }
}

fn render_selected_symbol_info(ui: &mut Ui, entry: &KicadSymbolEntry, state: &mut ViewerState) {
    ui.separator();
    ui.strong(&entry.name);
    if entry.units.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.label("Unit:");
            for &unit in &entry.units {
                if ui
                    .selectable_label(state.selected_unit == unit, format!("{unit}"))
                    .clicked()
                    && state.selected_unit != unit
                {
                    state.selected_unit = unit;
                }
            }
        });
    }
    for (k, v) in &entry.properties {
        if !v.is_empty() {
            ui.label(format!("{k}: {v}"));
        }
    }
    ui.separator();
    ui.strong(format!("Pins ({})", entry.pins.len()));
    for pin in entry.pins.iter().take(128) {
        ui.label(format!(
            "#{} {} [{}] (U{})",
            pin.number, pin.name, pin.pin_type, pin.unit
        ));
    }
}

pub(super) fn render_gbrjob_details(ui: &mut Ui, job: &GerberJobDocument) {
    ui.strong(format!("Gerber Job · {}", job.project_name));
    if let Some([x, y]) = job.board_size_mm {
        ui.label(format!("Board size: {x:.2} × {y:.2} mm"));
    }
    if let Some(layers) = job.layer_count {
        ui.label(format!("Layers: {layers}"));
    }
    if let Some(t) = job.board_thickness_mm {
        ui.label(format!("Thickness: {t:.2} mm"));
    }
    ui.label(format!("Surface finish: {}", job.finish));
    if !job.generator.is_empty() {
        ui.label(format!("Generator: {}", job.generator));
    }

    if !job.stackup.is_empty() {
        ui.separator();
        ui.strong(format!("Material Stackup ({})", job.stackup.len()));
        for layer in &job.stackup {
            let [r, g, b, _] = stackup_layer_rgba(layer);
            ui.horizontal(|ui| {
                let (swatch, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(swatch, 2.0, Color32::from_rgb(r, g, b));
                let thick = layer
                    .thickness_mm
                    .map_or_else(String::new, |t| format!(" · {t:.3} mm"));
                let mat = if layer.material.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", layer.material)
                };
                ui.add(
                    egui::Label::new(format!(
                        "{}: {}{}{thick}",
                        layer.name, layer.layer_type, mat
                    ))
                    .wrap(),
                );
            });
        }
    }

    if !job.files.is_empty() {
        ui.separator();
        ui.strong(format!("Layer-to-File Mapping ({})", job.files.len()));
        for f in &job.files {
            ui.add(
                egui::Label::new(format!(
                    "{} -> {} ({})",
                    f.file_function, f.path, f.file_polarity
                ))
                .wrap(),
            );
        }
    }
}

pub(super) fn render_project_details(ui: &mut Ui, proj: &KicadProjectDocument) {
    ui.strong(format!("KiCad Project · {}", proj.project_name));
    if let Some(v) = proj.meta_version {
        ui.label(format!("Schema version: {v}"));
    }
    if let Some(t) = proj.min_track_width_mm {
        ui.label(format!("Min track width: {t:.3} mm"));
    }
    if let Some(c) = proj.min_clearance_mm {
        ui.label(format!("Min clearance: {c:.3} mm"));
    }
    if let Some(v) = proj.min_via_diameter_mm {
        ui.label(format!("Min via diameter: {v:.3} mm"));
    }
    ui.label(format!(
        "Pinned libraries: {} symbol, {} footprint",
        proj.pinned_symbol_libs, proj.pinned_footprint_libs
    ));

    if !proj.net_classes.is_empty() {
        ui.separator();
        ui.strong(format!("Net Classes ({})", proj.net_classes.len()));
        for nc in &proj.net_classes {
            let clr = nc
                .clearance_mm
                .map_or_else(|| "—".into(), |v| format!("{v:.3}"));
            let trk = nc
                .track_width_mm
                .map_or_else(|| "—".into(), |v| format!("{v:.3}"));
            let via = nc
                .via_diameter_mm
                .map_or_else(|| "—".into(), |v| format!("{v:.3}"));
            ui.label(format!("{}: clr {clr} · trk {trk} · via {via} mm", nc.name));
        }
    }

    if !proj.text_vars.is_empty() {
        ui.separator();
        ui.strong("Text Variables");
        for (k, v) in &proj.text_vars {
            ui.label(format!("{k} = {v}"));
        }
    }
}
