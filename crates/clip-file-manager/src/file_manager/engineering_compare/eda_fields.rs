//! EDA records that are useful to compare by identity in the shared field table.

use super::Facts;
use little_clip_core::eda::{Document, EdaFormat, spice::SpicePreviewSample};
use std::collections::BTreeMap;

pub(super) fn supported(format: EdaFormat) -> bool {
    matches!(
        format,
        EdaFormat::PickAndPlace
            | EdaFormat::GerberJob
            | EdaFormat::KicadProject
            | EdaFormat::KicadPcb
            | EdaFormat::KicadFootprint
            | EdaFormat::KicadSchematic
            | EdaFormat::KicadSymbolLib
            | EdaFormat::SpiceNetlist
            | EdaFormat::LtspicePlot
            | EdaFormat::SpiceRaw
    )
}

pub(super) fn prepare(facts: &mut Facts, document: &Document) {
    match document {
        Document::PickAndPlace(doc) => grouped(
            facts,
            doc.placements.iter().map(|part| {
                (
                    format!("Part {}", part.ref_des),
                    format!(
                        "{} · {} · {} · ({}, {}) mm · {}°",
                        part.value,
                        part.package,
                        part.side_raw,
                        part.x_mm,
                        part.y_mm,
                        part.rotation_deg
                    ),
                )
            }),
        ),
        Document::KicadProject(doc) => {
            for (key, value) in &doc.design_rules {
                facts.add(&format!("Rule · {key}"), value);
            }
            for (key, value) in &doc.text_vars {
                facts.add(&format!("Text variable · {key}"), value);
            }
            grouped(
                facts,
                doc.net_classes.iter().map(|class| {
                    (
                        format!("Net class {}", class.name),
                        format!(
                            "clearance {} · width {} · via {} · drill {}",
                            mm(class.clearance_mm),
                            mm(class.track_width_mm),
                            mm(class.via_diameter_mm),
                            mm(class.via_drill_mm)
                        ),
                    )
                }),
            );
        }
        Document::GerberJob(doc) => {
            for (key, value) in &doc.design_rules {
                facts.add(&format!("Rule · {key}"), value);
            }
            for (index, layer) in doc.stackup.iter().enumerate() {
                facts.add(
                    &format!("Stackup layer {}", index + 1),
                    format!(
                        "{} · {} · {} · {} · {} · εr {:?} · loss {:?}",
                        layer.layer_type,
                        layer.name,
                        layer.color,
                        layer.material,
                        mm(layer.thickness_mm),
                        layer.epsilon_r,
                        layer.loss_tangent
                    ),
                );
            }
            grouped(
                facts,
                doc.files.iter().map(|file| {
                    (
                        format!("Fabrication layer {}", file.file_function),
                        format!("{} · {}", file.path, file.file_polarity),
                    )
                }),
            );
        }
        Document::KicadPcb(doc) => {
            facts.add("Track segments", doc.track_segment_count);
            facts.add("Track arcs", doc.track_arc_count);
            facts.add("Track length (mm)", doc.track_length_mm);
            facts.add("Vias", doc.via_count);
            facts.add("Zones", doc.zone_count);
            observed(
                facts,
                "footprints",
                doc.footprints.len(),
                doc.footprint_count,
            );
            observed(facts, "nets", doc.nets.len(), doc.net_count);
            grouped(
                facts,
                doc.footprints.iter().map(|part| {
                    (
                        format!("Footprint {}", part.ref_des),
                        format!(
                            "{} · {} · {} · {:?} mm · {}° · {} pads",
                            part.value,
                            part.lib_id,
                            part.layer,
                            part.pos_mm,
                            part.rot_deg,
                            part.pad_count
                        ),
                    )
                }),
            );
            for net in &doc.nets {
                facts.add(&format!("Net {}", net.name), "Present");
            }
        }
        Document::KicadFootprint(doc) => {
            facts.add("Footprint layer", &doc.layer);
            facts.add("Attributes", doc.attributes.join(" · "));
            facts.add("3D models", doc.models_3d.join(" · "));
            grouped(
                facts,
                doc.pads.iter().map(|pad| {
                    (
                        format!("Pad {}", pad.number),
                        format!(
                            "{} · {} · {} · {:?} mm · size {:?} mm · drill {:?}",
                            pad.pad_type,
                            pad.shape,
                            pad.layers,
                            pad.pos_mm,
                            pad.size_mm,
                            pad.drill_mm
                        ),
                    )
                }),
            );
        }
        Document::KicadSchematic(doc) => {
            facts.add("Junctions", doc.junction_count);
            facts.add("No-connect markers", doc.no_connect_count);
            observed(facts, "symbols", doc.symbols.len(), doc.symbol_count);
            grouped(
                facts,
                doc.symbols.iter().map(|symbol| {
                    (
                        format!("Symbol {} · unit {}", symbol.reference, symbol.unit),
                        format!(
                            "{} · {} · {} · {:?} mm · {}° · mirror {}",
                            symbol.value,
                            symbol.lib_id,
                            symbol.footprint,
                            symbol.pos_mm,
                            symbol.rotation_deg,
                            symbol.mirror
                        ),
                    )
                }),
            );
            grouped(
                facts,
                doc.net_labels.iter().map(|label| {
                    (
                        format!("{} label {}", label.kind.label(), label.name),
                        format!("{} · {} appearance(s)", label.shape, label.occurrences),
                    )
                }),
            );
            for sheet in &doc.sheets {
                facts.add(&format!("Sheet {}", sheet.name), &sheet.filename);
            }
        }
        Document::KicadSymbolLib(doc) => {
            for symbol in &doc.symbols {
                let name = &symbol.name;
                facts.add(
                    &format!("Symbol {name}"),
                    format!(
                        "{} · {} · {} · units {:?}",
                        symbol.reference_prefix, symbol.value, symbol.footprint, symbol.units,
                    ),
                );
                grouped(
                    facts,
                    symbol.pins.iter().map(|pin| {
                        (
                            format!("Symbol {name} · unit {} · pin {}", pin.unit, pin.number),
                            format!("{} · {}", pin.name, pin.pin_type),
                        )
                    }),
                );
            }
        }
        Document::SpiceNetlist(doc) => {
            observed(
                facts,
                "components",
                doc.components.len(),
                doc.component_count,
            );
            grouped(
                facts,
                doc.components.iter().map(|part| {
                    (
                        format!("Element {}", part.name.to_ascii_lowercase()),
                        format!("{} · {}", part.nodes.join(" "), part.card),
                    )
                }),
            );
            grouped(
                facts,
                doc.analyses.iter().map(|card| {
                    (
                        format!(
                            "Analysis {}",
                            card.split_whitespace()
                                .next()
                                .unwrap_or_default()
                                .to_ascii_lowercase()
                        ),
                        card.clone(),
                    )
                }),
            );
            grouped(
                facts,
                doc.models.iter().map(|model| {
                    (
                        format!("Model {}", model.name.to_ascii_lowercase()),
                        model.kind.clone(),
                    )
                }),
            );
            grouped(
                facts,
                doc.subcircuits.iter().map(|sub| {
                    (
                        format!("Subcircuit {}", sub.name.to_ascii_lowercase()),
                        format!("{} · {} elements", sub.ports.join(" "), sub.element_count),
                    )
                }),
            );
        }
        Document::LtspicePlot(doc) => {
            let mut occurrence = BTreeMap::new();
            for section in &doc.sections {
                let ordinal = occurrence.entry(&section.analysis).or_insert(0);
                *ordinal += 1;
                for (pane, config) in section.panes.iter().enumerate() {
                    facts.add(
                        &format!("{} {} · pane {}", section.analysis, ordinal, pane + 1),
                        format!(
                            "log X {} · log Y {} · {}",
                            config.log_x,
                            config.log_y,
                            config.traces.join(" · ")
                        ),
                    );
                }
            }
        }
        Document::SpiceRaw(doc) => {
            let mut occurrences = BTreeMap::new();
            for plot in &doc.plots {
                let identity = (plot.kind.label(), plot.plot_name.to_ascii_lowercase());
                let ordinal = occurrences.entry(identity.clone()).or_insert(0);
                *ordinal += 1;
                let plot_key = format!("{} · {} · {}", identity.0, identity.1, ordinal);
                facts.add(&format!("{plot_key} · source rows"), plot.total_points);
                observed(
                    facts,
                    &format!("{plot_key} · signals"),
                    plot.traces.len(),
                    plot.total_vars
                        .saturating_sub(usize::from(!plot.x_name.is_empty())),
                );
                let mut traces = BTreeMap::new();
                for trace in &plot.traces {
                    let name = trace.name.to_ascii_lowercase();
                    let ordinal = traces.entry(name.clone()).or_insert(0);
                    *ordinal += 1;
                    for (index, sweep) in trace.sweeps().enumerate() {
                        let key =
                            format!("{plot_key} · {name} · {} · sweep {}", ordinal, index + 1);
                        facts.add(
                            &key,
                            sweep_field(sweep, &plot.x_unit, &trace.unit_type, plot.is_complex),
                        );
                    }
                }
            }
        }
        Document::LtspiceSchematic(doc) => {
            facts.add("Wire count", doc.wire_count);
            facts.add("Placed symbols", doc.symbol_count);
            facts.add("Unresolved symbols", doc.unresolved.join(" · "));
        }
        _ => {}
    }
}

fn grouped(facts: &mut Facts, entries: impl Iterator<Item = (String, String)>) {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for (key, value) in entries {
        groups.entry(key).or_default().push(value);
    }
    for (key, mut values) in groups {
        if values.len() == 1 {
            facts.add(&key, &values[0]);
        } else {
            values.sort_unstable();
            facts.add(&format!("{key} · occurrences"), values.len());
            for (index, value) in values.into_iter().enumerate() {
                facts.add(&format!("{key} · {}", index + 1), value);
            }
        }
    }
}

fn mm(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value} mm"))
}

fn observed(facts: &mut Facts, name: &str, retained: usize, total: usize) {
    if retained < total {
        facts.add(
            &format!("Preview coverage · {name}"),
            format!("{retained} of {total}"),
        );
    }
}

fn sweep_field(samples: &[SpicePreviewSample], x_unit: &str, unit: &str, complex: bool) -> String {
    let Some((first, last)) = samples.first().zip(samples.last()) else {
        return "No finite samples retained".to_owned();
    };
    let mut lo = first.value;
    let mut hi = first.value;
    let mut phase_lo = first.phase_deg;
    let mut phase_hi = first.phase_deg;
    for sample in samples {
        lo = lo.min(sample.value);
        hi = hi.max(sample.value);
        phase_lo = phase_lo.min(sample.phase_deg);
        phase_hi = phase_hi.max(sample.phase_deg);
    }
    if complex {
        format!(
            "{} → {} {x_unit} · {lo} to {hi} dB · {phase_lo} to {phase_hi}°",
            first.x, last.x
        )
    } else {
        format!("{} → {} {x_unit} · {lo} to {hi} {unit}", first.x, last.x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::eda::parse_bytes;
    use std::{path::Path, sync::atomic::AtomicBool};

    fn fields(path: &str, input: &str) -> Facts {
        let document =
            parse_bytes(Path::new(path), input.as_bytes(), &AtomicBool::new(false)).unwrap();
        let mut facts = Facts::default();
        prepare(&mut facts, &document);
        facts
    }

    #[test]
    fn placement_fields_match_reference_across_reordering_and_report_moves() {
        let header = "Ref,Val,Package,PosX,PosY,Rot,Side\n";
        let a = fields(
            "a.pos",
            &format!("{header}R1,1k,R_0603,10,20,0,top\nC1,1u,C_0603,11,20,0,top\n"),
        );
        let b = fields(
            "b.pos",
            &format!("{header}C1,1u,C_0603,11,20,0,top\nR1,1k,R_0603,10,20,0,top\n"),
        );
        assert_eq!(a.0, b.0);
        let moved = fields(
            "b.pos",
            &format!("{header}C1,1u,C_0603,11,20,0,top\nR1,1k,R_0603,12,20,90,bottom\n"),
        );
        assert_eq!(super::super::diff_keys(&a, &moved, false), ["Part R1"]);
    }

    #[test]
    fn circuit_fields_report_value_changes_without_calling_a_solver() {
        let a = fields("a.cir", "Divider\nR1 a b 2k\nR2 b 0 1k\n.op\n.end\n");
        let b = fields("b.cir", "Divider\nR2 b 0 1k\nR1 a b 3k\n.op\n.end\n");
        assert_eq!(super::super::diff_keys(&a, &b, false), ["Element r1"]);
        assert!(a.0["Element r1"].contains("2k"));
        assert!(b.0["Element r1"].contains("3k"));
    }
}
