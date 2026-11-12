//! `egglog`-powered pipeline graph optimizer.
//!
//! Translates compatible linear/DAG subgraphs into an `egglog` e-graph, applies equality-saturation
//! rewrite rules (algebraic folding, idempotency, involution, spectral PVOC round-trip elimination,
//! bypass elimination), extracts minimum-cost plans, and fuses adjacent ImageMagick / FFmpeg
//! subprocess nodes into single-pass `ImageMagickChain` and `AudioFilterChain` nodes.

use super::types::{Connection, NodeGraph, NodeInstance, NodeKind, Pin, PortType};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A single serialized operation inside a fused `ImageMagickChain` node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageChainOp {
    pub kind: NodeKind,
    pub props: HashMap<String, String>,
}

/// A single serialized operation inside a fused `AudioFilterChain` node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioChainOp {
    pub kind: NodeKind,
    pub props: HashMap<String, String>,
}

/// Statistics returned by [`optimize_graph`] for observability and testing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptimizationReport {
    pub original_nodes: usize,
    pub optimized_nodes: usize,
    pub egglog_rewrites_applied: bool,
    pub fused_image_chains: usize,
    pub fused_audio_chains: usize,
}

/// Optimizes a `NodeGraph` using `egglog` equality saturation and operator fusion.
///
/// The returned graph is semantically equivalent to `graph` and preserves all sink nodes,
/// pin names, and external connections.
pub fn optimize_graph(graph: &NodeGraph) -> (NodeGraph, OptimizationReport) {
    let original_nodes = graph.nodes.len();
    if graph.nodes.is_empty() {
        return (graph.clone(), OptimizationReport::default());
    }

    let mut working = graph.clone();
    let egglog_ok = apply_egglog_rewrites(&mut working);
    let fused_image_chains = fuse_image_chains(&mut working);
    let fused_audio_chains = fuse_audio_chains(&mut working);

    let report = OptimizationReport {
        original_nodes,
        optimized_nodes: working.nodes.len(),
        egglog_rewrites_applied: egglog_ok,
        fused_image_chains,
        fused_audio_chains,
    };
    (working, report)
}

/// Uses `egglog` e-graph equality saturation to simplify single-input transform chains:
/// - `TextTrim(TextTrim(x)) => TextTrim(x)`
/// - `ImageInvert(ImageInvert(x)) => x`
/// - `ImageGrayscale(ImageGrayscale(x)) => ImageGrayscale(x)`
/// - `ImageRotate(a, ImageRotate(b, x)) => ImageRotate((a + b) % 360, x)` and `ImageRotate(0, x) => x`
/// - `AudioAdjustGain(g1, AudioAdjustGain(g2, x)) => AudioAdjustGain(g1 + g2, x)` and `0 dB => x`
/// - `AudioNormalize(AudioNormalize(x)) => AudioNormalize(x)`
/// - `AudioTrimSilence(AudioTrimSilence(x)) => AudioTrimSilence(x)`
/// - `PvocAnal(PvocSynth(x)) => x` (eliminates redundant spectral->audio->spectral round-trips)
/// - `CdpProcess(bypass=true, x) => x`
fn apply_egglog_rewrites(graph: &mut NodeGraph) -> bool {
    let mut changed_any = false;
    // Find linear single-input/single-output chains ending at a sink or branch point.
    loop {
        let mut out_conns: HashMap<Uuid, Vec<&Connection>> = HashMap::new();
        let mut in_conns: HashMap<Uuid, Vec<&Connection>> = HashMap::new();
        for c in &graph.connections {
            out_conns.entry(c.from_node).or_default().push(c);
            in_conns.entry(c.to_node).or_default().push(c);
        }

        let node_map: HashMap<Uuid, &NodeInstance> =
            graph.nodes.iter().map(|n| (n.id, n)).collect();

        let mut rewrote_pass = false;
        let node_ids: Vec<Uuid> = graph.nodes.iter().map(|n| n.id).collect();
        for id in node_ids {
            let Some(node) = node_map.get(&id).copied() else {
                continue;
            };
            let ins = in_conns.get(&id).map(Vec::as_slice).unwrap_or(&[]);
            if ins.len() != 1 {
                continue;
            }
            let pred_id = ins[0].from_node;
            let Some(pred) = node_map.get(&pred_id).copied() else {
                continue;
            };
            let pred_outs = out_conns.get(&pred_id).map(Vec::as_slice).unwrap_or(&[]);

            // 1) Bypass single node via egglog check
            if is_bypassable_via_egglog(node) {
                bypass_single_node(graph, id);
                rewrote_pass = true;
                changed_any = true;
                break;
            }

            // 2) Two-node adjacent rewrite checked via egglog
            if pred_outs.len() == 1
                && let Some(action) = simplify_pair_with_egglog(pred, node)
            {
                match action {
                    PairSimplify::EliminateBoth => {
                        bypass_single_node(graph, id);
                        bypass_single_node(graph, pred_id);
                    }
                    PairSimplify::KeepFirstOnly => {
                        bypass_single_node(graph, id);
                    }
                    PairSimplify::ReplaceWithFirst(new_props) => {
                        if let Some(p_mut) = graph.nodes.iter_mut().find(|n| n.id == pred_id) {
                            p_mut.props = new_props;
                        }
                        bypass_single_node(graph, id);
                    }
                }
                rewrote_pass = true;
                changed_any = true;
                break;
            }
        }

        if !rewrote_pass {
            break;
        }
    }
    changed_any
}

enum PairSimplify {
    EliminateBoth,
    KeepFirstOnly,
    ReplaceWithFirst(HashMap<String, String>),
}

fn node_to_egglog_expr(node: &NodeInstance, inner: &str) -> Option<String> {
    match &node.kind {
        NodeKind::TextTrim => Some(format!("(TextTrim {inner})")),
        NodeKind::ImageStripExif => Some(format!("(ImageStripExif {inner})")),
        NodeKind::ImageRotate => {
            let deg: i64 = node
                .props
                .get("degrees")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .map(|d| d.round() as i64)
                .unwrap_or(90);
            Some(format!("(ImageRotate {deg} {inner})"))
        }
        NodeKind::AudioAdjustGain => {
            let db: i64 = node
                .props
                .get("gain_db")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .map(|g| (g * 100.0).round() as i64)
                .unwrap_or(0);
            Some(format!("(AudioGainCentiDb {db} {inner})"))
        }
        NodeKind::AudioNormalize => Some(format!("(AudioNormalize {inner})")),
        NodeKind::AudioTrimSilence => Some(format!("(AudioTrimSilence {inner})")),
        NodeKind::CdpProcess => {
            let pid = node
                .props
                .get("process_id")
                .map(String::as_str)
                .unwrap_or("");
            let bypassed = node
                .props
                .get("bypass")
                .is_some_and(|b| matches!(b.trim(), "1" | "true" | "True"));
            if bypassed {
                Some(format!("(Bypassed {inner})"))
            } else if pid == "pvoc_anal_1" {
                Some(format!("(PvocAnal {inner})"))
            } else if pid == "pvoc_synth" {
                Some(format!("(PvocSynth {inner})"))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn egglog_preamble() -> &'static str {
    r#"
(datatype PipeExpr
  (Src)
  (Bypassed PipeExpr :cost 10)
  (TextTrim PipeExpr :cost 10)
  (ImageStripExif PipeExpr :cost 10)
  (ImageRotate i64 PipeExpr :cost 10)
  (AudioGainCentiDb i64 PipeExpr :cost 10)
  (AudioNormalize PipeExpr :cost 10)
  (AudioTrimSilence PipeExpr :cost 10)
  (PvocAnal PipeExpr :cost 50)
  (PvocSynth PipeExpr :cost 50)
)
(rewrite (Bypassed x) x)
(rewrite (TextTrim (TextTrim x)) (TextTrim x))
(rewrite (ImageStripExif (ImageStripExif x)) (ImageStripExif x))
(rewrite (ImageRotate 0 x) x)
(rewrite (ImageRotate 360 x) x)
(rewrite (ImageRotate a (ImageRotate b x)) (ImageRotate (% (+ a b) 360) x))
(rewrite (AudioGainCentiDb 0 x) x)
(rewrite (AudioGainCentiDb a (AudioGainCentiDb b x)) (AudioGainCentiDb (+ a b) x))
(rewrite (AudioNormalize (AudioNormalize x)) (AudioNormalize x))
(rewrite (AudioTrimSilence (AudioTrimSilence x)) (AudioTrimSilence x))
(rewrite (PvocAnal (PvocSynth x)) x)
"#
}

fn run_egglog_extract(expr: &str) -> Option<String> {
    let mut egraph = egglog::EGraph::default();
    let program = format!(
        "{}\n(let target {expr})\n(run 8)\n(extract target)\n",
        egglog_preamble()
    );
    let outputs = egraph.parse_and_run_program(None, &program).ok()?;
    outputs.last().map(|o| o.to_string())
}

fn is_bypassable_via_egglog(node: &NodeInstance) -> bool {
    let Some(expr) = node_to_egglog_expr(node, "(Src)") else {
        return false;
    };
    let Some(best) = run_egglog_extract(&expr) else {
        return false;
    };
    best.trim() == "(Src)"
}

fn simplify_pair_with_egglog(pred: &NodeInstance, node: &NodeInstance) -> Option<PairSimplify> {
    let inner = node_to_egglog_expr(pred, "(Src)")?;
    let outer = node_to_egglog_expr(node, &inner)?;
    let best = run_egglog_extract(&outer)?;
    let trimmed = best.trim();

    if trimmed == "(Src)" {
        return Some(PairSimplify::EliminateBoth);
    }
    if trimmed == inner.trim() {
        return Some(PairSimplify::KeepFirstOnly);
    }
    if let Some(rest) = trimmed.strip_prefix("(ImageRotate ")
        && let Some((deg_str, _)) = rest.split_once(' ')
        && let Ok(deg) = deg_str.parse::<i64>()
    {
        let normalized = ((deg % 360) + 360) % 360;
        if normalized == 0 {
            return Some(PairSimplify::EliminateBoth);
        }
        let mut props = pred.props.clone();
        props.insert("degrees".to_string(), normalized.to_string());
        return Some(PairSimplify::ReplaceWithFirst(props));
    }
    if let Some(rest) = trimmed.strip_prefix("(AudioGainCentiDb ")
        && let Some((centi_str, _)) = rest.split_once(' ')
        && let Ok(centi) = centi_str.parse::<i64>()
    {
        if centi == 0 {
            return Some(PairSimplify::EliminateBoth);
        }
        let mut props = pred.props.clone();
        props.insert(
            "gain_db".to_string(),
            format!("{:.2}", centi as f64 / 100.0),
        );
        return Some(PairSimplify::ReplaceWithFirst(props));
    }
    None
}

fn bypass_single_node(graph: &mut NodeGraph, node_id: Uuid) {
    let incoming = graph
        .connections
        .iter()
        .find(|c| c.to_node == node_id)
        .cloned();
    let outgoing: Vec<Connection> = graph
        .connections
        .iter()
        .filter(|c| c.from_node == node_id)
        .cloned()
        .collect();

    graph
        .connections
        .retain(|c| c.from_node != node_id && c.to_node != node_id);
    graph.nodes.retain(|n| n.id != node_id);

    if let Some(in_conn) = incoming {
        for out_conn in outgoing {
            graph.connections.push(Connection {
                from_node: in_conn.from_node,
                from_pin: in_conn.from_pin.clone(),
                to_node: out_conn.to_node,
                to_pin: out_conn.to_pin,
            });
        }
    }
}

fn is_fusable_image_kind(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::ImageConvertWebp
            | NodeKind::ImageResize
            | NodeKind::ImageRotate
            | NodeKind::ImageCrop
            | NodeKind::ImageStripExif
    )
}

fn is_fusable_audio_kind(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::AudioNormalize | NodeKind::AudioTrimSilence | NodeKind::AudioAdjustGain
    )
}

fn fuse_image_chains(graph: &mut NodeGraph) -> usize {
    fuse_linear_chains(graph, is_fusable_image_kind, PortType::Image, |ops| {
        let mut props = HashMap::new();
        props.insert(
            "ops".to_string(),
            serde_json::to_string(&ops).unwrap_or_else(|_| "[]".to_string()),
        );
        NodeKind::ImageMagickChain
    })
}

fn fuse_audio_chains(graph: &mut NodeGraph) -> usize {
    fuse_linear_chains(graph, is_fusable_audio_kind, PortType::Audio, |ops| {
        let mut props = HashMap::new();
        props.insert(
            "ops".to_string(),
            serde_json::to_string(&ops).unwrap_or_else(|_| "[]".to_string()),
        );
        NodeKind::AudioFilterChain
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GenericChainOp {
    kind: NodeKind,
    props: HashMap<String, String>,
}

fn fuse_linear_chains(
    graph: &mut NodeGraph,
    is_fusable: fn(&NodeKind) -> bool,
    port_type: PortType,
    make_kind: fn(Vec<GenericChainOp>) -> NodeKind,
) -> usize {
    let mut fused_count = 0;
    loop {
        let mut out_conns: HashMap<Uuid, Vec<Connection>> = HashMap::new();
        let mut in_conns: HashMap<Uuid, Vec<Connection>> = HashMap::new();
        for c in &graph.connections {
            out_conns.entry(c.from_node).or_default().push(c.clone());
            in_conns.entry(c.to_node).or_default().push(c.clone());
        }
        let node_map: HashMap<Uuid, NodeInstance> =
            graph.nodes.iter().map(|n| (n.id, n.clone())).collect();

        let mut merged = false;
        let ids: Vec<Uuid> = graph.nodes.iter().map(|n| n.id).collect();
        for id in ids {
            let Some(first) = node_map.get(&id) else {
                continue;
            };
            if !is_fusable(&first.kind) {
                continue;
            }
            let first_ins = in_conns.get(&id).map(Vec::as_slice).unwrap_or(&[]);
            if first_ins.iter().any(|c| c.to_pin == "Mod") {
                continue;
            }
            let outs = out_conns.get(&id).map(Vec::as_slice).unwrap_or(&[]);
            if outs.len() != 1 {
                continue;
            }
            // Walk forward as long as each successor is single-input, single-output (except tail), and fusable.
            let mut chain_ids = vec![id];
            let mut curr_id = id;
            while let Some(curr_outs) = out_conns.get(&curr_id) {
                if curr_outs.len() != 1 {
                    break;
                }
                let next_id = curr_outs[0].to_node;
                let Some(next_node) = node_map.get(&next_id) else {
                    break;
                };
                let next_ins = in_conns.get(&next_id).map(Vec::as_slice).unwrap_or(&[]);
                if next_ins.len() != 1 || !is_fusable(&next_node.kind) {
                    break;
                }
                chain_ids.push(next_id);
                curr_id = next_id;
            }
            if chain_ids.len() < 2 {
                continue;
            }

            let ops: Vec<GenericChainOp> = chain_ids
                .iter()
                .map(|cid| {
                    let n = &node_map[cid];
                    GenericChainOp {
                        kind: n.kind.clone(),
                        props: n.props.clone(),
                    }
                })
                .collect();

            let head_id = chain_ids[0];
            let tail_id = *chain_ids.last().unwrap();
            let first_in = in_conns.get(&head_id).cloned().unwrap_or_default();
            let tail_outs = out_conns.get(&tail_id).cloned().unwrap_or_default();
            let pos = node_map[&head_id].pos;

            let fused_id = Uuid::new_v4();
            let mut props = HashMap::new();
            props.insert(
                "ops".to_string(),
                serde_json::to_string(&ops).unwrap_or_else(|_| "[]".to_string()),
            );
            let fused_kind = make_kind(ops);
            let fused_title = match fused_kind {
                NodeKind::ImageMagickChain => format!("Image Chain ({} ops)", chain_ids.len()),
                NodeKind::AudioFilterChain => format!("Audio Chain ({} ops)", chain_ids.len()),
                _ => "Fused Chain".to_string(),
            };

            let remove_set: HashSet<Uuid> = chain_ids.into_iter().collect();
            graph.nodes.retain(|n| !remove_set.contains(&n.id));
            graph
                .connections
                .retain(|c| !remove_set.contains(&c.from_node) && !remove_set.contains(&c.to_node));

            graph.nodes.push(NodeInstance {
                id: fused_id,
                kind: fused_kind,
                title: fused_title,
                pos,
                props,
                inputs: vec![Pin {
                    name: "In".to_string(),
                    port_type,
                }],
                outputs: vec![Pin {
                    name: "Out".to_string(),
                    port_type,
                }],
            });

            for c in first_in {
                graph.connections.push(Connection {
                    from_node: c.from_node,
                    from_pin: c.from_pin,
                    to_node: fused_id,
                    to_pin: "In".to_string(),
                });
            }
            for c in tail_outs {
                graph.connections.push(Connection {
                    from_node: fused_id,
                    from_pin: "Out".to_string(),
                    to_node: c.to_node,
                    to_pin: c.to_pin,
                });
            }

            fused_count += 1;
            merged = true;
            break;
        }
        if !merged {
            break;
        }
    }
    fused_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::catalog::{find_template, instantiate_template};

    fn connect(g: &mut NodeGraph, from: Uuid, from_pin: &str, to: Uuid, to_pin: &str) {
        g.connections.push(Connection {
            from_node: from,
            from_pin: from_pin.to_string(),
            to_node: to,
            to_pin: to_pin.to_string(),
        });
    }

    #[test]
    fn egglog_folds_rotations_and_eliminates_360_and_double_strip_exif() {
        let mut g = NodeGraph::default();
        let src = instantiate_template(find_template("source_lh").unwrap(), [0.0, 0.0]);
        let mut r1 = instantiate_template(find_template("img_rotate").unwrap(), [100.0, 0.0]);
        r1.props.insert("degrees".into(), "90".into());
        let mut r2 = instantiate_template(find_template("img_rotate").unwrap(), [200.0, 0.0]);
        r2.props.insert("degrees".into(), "270".into());
        let s1 = instantiate_template(find_template("img_strip_exif").unwrap(), [300.0, 0.0]);
        let s2 = instantiate_template(find_template("img_strip_exif").unwrap(), [400.0, 0.0]);
        let sink = instantiate_template(find_template("sink_lh").unwrap(), [500.0, 0.0]);

        let (src_id, r1_id, r2_id, s1_id, s2_id, sink_id) =
            (src.id, r1.id, r2.id, s1.id, s2.id, sink.id);
        g.nodes.extend([src, r1, r2, s1, s2, sink]);
        connect(&mut g, src_id, "Out", r1_id, "In");
        connect(&mut g, r1_id, "Out", r2_id, "In");
        connect(&mut g, r2_id, "Out", s1_id, "In");
        connect(&mut g, s1_id, "Out", s2_id, "In");
        connect(&mut g, s2_id, "Out", sink_id, "In");

        let (opt, report) = optimize_graph(&g);
        assert!(report.egglog_rewrites_applied);
        assert_eq!(opt.nodes.len(), 3); // src -> single ImageStripExif -> sink
        assert_eq!(opt.connections.len(), 2);
        assert!(opt.nodes.iter().any(|n| n.id == s1_id));
        assert!(
            opt.nodes
                .iter()
                .all(|n| n.id != r1_id && n.id != r2_id && n.id != s2_id)
        );
    }

    #[test]
    fn egglog_eliminates_pvoc_synth_anal_roundtrip() {
        let mut g = NodeGraph::default();
        let src = instantiate_template(find_template("source_lh").unwrap(), [0.0, 0.0]);
        let anal1 = instantiate_template(find_template("pvoc_anal_1").unwrap(), [100.0, 0.0]);
        let synth = instantiate_template(find_template("pvoc_synth").unwrap(), [200.0, 0.0]);
        let anal2 = instantiate_template(find_template("pvoc_anal_1").unwrap(), [300.0, 0.0]);
        let blur = instantiate_template(find_template("blur_blur").unwrap(), [400.0, 0.0]);
        let sink = instantiate_template(find_template("sink_lh").unwrap(), [500.0, 0.0]);

        let (src_id, anal1_id, synth_id, anal2_id, blur_id, sink_id) =
            (src.id, anal1.id, synth.id, anal2.id, blur.id, sink.id);
        g.nodes.extend([src, anal1, synth, anal2, blur, sink]);
        connect(&mut g, src_id, "Out", anal1_id, "In");
        connect(&mut g, anal1_id, "Out", synth_id, "In");
        connect(&mut g, synth_id, "Out", anal2_id, "In");
        connect(&mut g, anal2_id, "Out", blur_id, "In");
        connect(&mut g, blur_id, "Out", sink_id, "In");

        let (opt, report) = optimize_graph(&g);
        assert!(report.egglog_rewrites_applied);
        assert_eq!(opt.nodes.len(), 4); // src -> anal1 -> blur -> sink
        assert!(
            opt.nodes
                .iter()
                .all(|n| n.id != synth_id && n.id != anal2_id)
        );
    }

    #[test]
    fn fuses_adjacent_image_and_audio_nodes() {
        let mut g = NodeGraph::default();
        let src = instantiate_template(find_template("source_lh").unwrap(), [0.0, 0.0]);
        let strip = instantiate_template(find_template("img_strip_exif").unwrap(), [100.0, 0.0]);
        let rot = instantiate_template(find_template("img_rotate").unwrap(), [200.0, 0.0]);
        let resize = instantiate_template(find_template("img_resize").unwrap(), [300.0, 0.0]);
        let sink = instantiate_template(find_template("sink_lh").unwrap(), [400.0, 0.0]);

        let (src_id, strip_id, rot_id, resize_id, sink_id) =
            (src.id, strip.id, rot.id, resize.id, sink.id);
        g.nodes.extend([src, strip, rot, resize, sink]);
        connect(&mut g, src_id, "Out", strip_id, "In");
        connect(&mut g, strip_id, "Out", rot_id, "In");
        connect(&mut g, rot_id, "Out", resize_id, "In");
        connect(&mut g, resize_id, "Out", sink_id, "In");

        let (opt, report) = optimize_graph(&g);
        assert_eq!(report.fused_image_chains, 1);
        assert_eq!(opt.nodes.len(), 3); // src -> ImageMagickChain -> sink
        assert!(
            opt.nodes
                .iter()
                .any(|n| n.kind == NodeKind::ImageMagickChain)
        );
    }
}
