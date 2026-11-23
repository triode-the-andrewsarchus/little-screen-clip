use std::{fs::File, io::Write, sync::atomic::AtomicBool};

use super::{Direction, Format, RateMeaning, Rates, State, StreamRate, identify, preview, read};

fn file(source: &str) -> File {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(source.as_bytes()).unwrap();
    file
}

#[test]
fn modern_formats_link_named_ports_and_keep_expressions_as_text() {
    let source = r#"options:
  parameters: {id: example, title: 'Filter graph'}
blocks:
- name: source
  id: analog_sig_source_x
  parameters:
    sample_rate: user_function(samp_rate)
  states: {coordinate: [18, 12.5], rotation: 0, state: enabled}
- name: filter
  id: low_pass_filter
  states: {state: bypassed}
- name: sink
  id: blocks_message_debug
connections:
- [source, 0, filter, '0']
- src_blk_id: filter
  src_port_id: 'out'
  snk_blk_id: sink
  snk_port_id: print
  params: {test_name: 'idle'}
- [sink, 0, source, 1]
- [sink, -1, source, 0]
metadata: {grc_version: '3.10.12', file_format: 2}
"#;
    assert_eq!(identify(source.as_bytes()), Some(Format::Yaml));
    let graph = read(file(source), &AtomicBool::new(false)).unwrap();
    assert_eq!(graph.title, "Filter graph");
    assert_eq!(
        graph.blocks[0].setting("sample_rate"),
        Some("user_function(samp_rate)")
    );
    assert_eq!(graph.blocks[0].position, Some([18.0, 12.5]));
    assert_eq!(graph.blocks[1].state, State::Bypassed);
    assert_eq!(graph.connections[1].source.port, "out");
    assert_eq!(graph.connections[1].settings[0].value, "idle");
    assert_eq!(graph.skipped_connections, 1);
    let visits = graph.trace(0, Direction::Downstream, 10);
    assert_eq!(
        visits
            .iter()
            .map(|step| (step.block, step.hops))
            .collect::<Vec<_>>(),
        [(1, 1), (2, 2)]
    );
    assert_eq!(graph.layout().positions[0], [18.0, 12.5]);
}

#[test]
fn legacy_layout_and_named_ports_survive_xml_read() {
    let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<?grc format="1" created="3.7.7"?>
<flow_graph>
  <block><key>options</key><param><key>id</key><value>legacy</value></param></block>
  <block><key>source</key><param><key>id</key><value>a</value></param>
    <param><key>_coordinate</key><value>(-20, 84.0)</value></param>
    <param><key>_enabled</key><value>False</value></param>
  </block>
  <block><key>sink</key><param><key>id</key><value>b</value></param></block>
  <connection><source_block_id>a</source_block_id><source_key>out</source_key>
    <sink_block_id>b</sink_block_id><sink_key>print</sink_key></connection>
</flow_graph>"#;
    let graph = read(file(source), &AtomicBool::new(false)).unwrap();
    assert_eq!(graph.format, Format::Xml);
    assert_eq!(graph.version.as_deref(), Some("3.7.7"));
    assert_eq!(graph.blocks[0].state, State::Disabled);
    assert_eq!(graph.layout().positions[0], [-20.0, 84.0]);
    assert_eq!(graph.connections[0].sink.port, "print");
}

#[test]
fn preview_recovers_a_bounded_prefix_and_excludes_ambiguous_wires() {
    let mut source = "options:\n  parameters: {id: large_example}\nblocks:\n- {name: a, id: variable}\n- {name: a, id: variable}\n- {name: b, id: variable}\nconnections:\n- [a, 0, b, 0]\nmetadata:\n  file_format: 1\n".to_owned();
    let pinned = AtomicBool::new(false);
    let graph = read(file(&source), &pinned).unwrap();
    assert_eq!(graph.skipped_blocks, 1);
    assert_eq!(graph.skipped_connections, 1);
    source.push('#');
    source.push_str(&"x".repeat(900_000));
    let preview = preview(file(&source), &pinned).unwrap();
    assert!(!preview.complete);
    assert_eq!(preview.graph.blocks.len(), 2);
    assert!(read(file(&source), &AtomicBool::new(true)).is_err());
}

#[test]
fn xml_dtd_does_not_resolve_custom_entities() {
    let source = r#"<!DOCTYPE flow_graph [<!ENTITY unexpected SYSTEM "file:///tmp/unused">]><flow_graph><block><key>&unexpected;</key></block></flow_graph>"#;
    assert!(read(file(source), &AtomicBool::new(false)).is_err());
}

#[test]
fn saved_sample_clocks_follow_variable_ratios_and_keep_the_display_scale_separate() {
    let source = r#"options:
  parameters: {id: rate_example}
blocks:
- {name: samp_rate, id: variable, parameters: {value: 'float(BASE) * 2'}}
- {name: BASE, id: variable_qtgui_label, parameters: {type: real, value: '4.8e4'}}
- {name: base, id: variable, parameters: {value: '250000'}}
- {name: tone, id: analog_sig_source_x, parameters: {samp_rate: samp_rate}}
- {name: filter, id: fir_filter_xxx, parameters: {decim: 4}}
- {name: resampler, id: rational_resampler_xxx, parameters: {interp: 2, decim: 3}}
- {name: spectrum, id: qtgui_freq_sink_x, parameters: {bw: '48e3'}}
connections:
- [tone, 0, filter, 0]
- [filter, 0, resampler, 0]
- [resampler, 0, spectrum, 0]
- [spectrum, freq, tone, command]
metadata: {file_format: 1}
"#;
    let graph = read(file(source), &AtomicBool::new(false)).unwrap();
    let rates = Rates::inspect(&graph);
    for (edge, expected) in [96_000.0, 24_000.0, 16_000.0].into_iter().enumerate() {
        let Some(StreamRate::Nominal(clock)) = rates.wire(edge) else {
            panic!("Missing stream rate on {edge}");
        };
        assert_eq!(clock.rate, expected);
        assert_eq!(clock.origin.unwrap().block, 3);
    }
    assert_eq!(rates.wire(3), None);
    let spectrum = rates.block(6).unwrap();
    let StreamRate::Nominal(input) = spectrum.input else {
        panic!("Missing the rate at the spectrum input");
    };
    let display = spectrum.setting.unwrap();
    assert_eq!(display.meaning, RateMeaning::Display);
    assert_eq!(display.rate, 48_000.0);
    assert!(display.differs(input));
}

#[test]
fn unresolved_expressions_and_conflicting_inputs_stay_visible() {
    let source = r#"options:
  parameters: {id: cautious_rates}
blocks:
- {name: a, id: variable, parameters: {value: b}}
- {name: b, id: variable, parameters: {value: a}}
- {name: cycle, id: analog_sig_source_x, parameters: {samp_rate: a}}
- {name: unknown, id: analog_sig_source_x, parameters: {samp_rate: 'get_rate()'}}
- {name: first, id: analog_sig_source_x, parameters: {samp_rate: 48000}}
- {name: second, id: analog_sig_source_x, parameters: {samp_rate: 96000}}
- {name: mix, id: blocks_add_xx}
- {name: unlabelled, id: analog_noise_source_x}
- {name: pace, id: blocks_throttle2, parameters: {samples_per_second: 12000}}
- {name: next, id: qtgui_time_sink_x, parameters: {srate: 24000}}
connections:
- [first, 0, mix, 0]
- [second, 0, mix, 1]
- [mix, 0, next, 0]
- [unlabelled, 0, pace, 0]
metadata: {file_format: 1}
"#;
    let graph = read(file(source), &AtomicBool::new(false)).unwrap();
    let rates = Rates::inspect(&graph);
    assert_eq!(rates.block(2).unwrap().output, StreamRate::Unknown);
    assert_eq!(rates.block(3).unwrap().output, StreamRate::Unknown);
    assert_eq!(rates.block(6).unwrap().input, StreamRate::Conflicting);
    assert_eq!(rates.wire(2), Some(StreamRate::Conflicting));
    let StreamRate::Nominal(paced) = rates.block(8).unwrap().output else {
        panic!("The throttle has an explicit pace");
    };
    assert_eq!(paced.rate, 12_000.0);
    assert_eq!(paced.origin.unwrap().meaning, RateMeaning::Pacing);
}
