use super::preview;
use little_clip_core::engineering::{
    radio::{self, BANDS},
    tags::Document,
    waveforms::display_value,
};
use std::{collections::BTreeMap, fmt::Display, fmt::Write, path::Path, sync::atomic::AtomicBool};

#[derive(Default)]
pub(super) struct Facts(pub BTreeMap<String, String>, usize);

impl Facts {
    pub fn prepare(data: &preview::Data, path: &Path, cancel: &AtomicBool) -> Self {
        let mut facts = Self::default();
        facts.add("Preview summary", &data.detail);
        if let Some(scope) = &data.scope {
            let overview = scope.overview();
            facts.add("Instrument", &overview.model);
            facts.add(
                "Capture duration",
                display_value(overview.duration_seconds(), "s"),
            );
            for (channel, envelope) in overview.channels.iter().zip(&overview.envelopes) {
                let peak = envelope.extrema();
                facts.add(
                    &format!("{} · sample rate", channel.name),
                    display_value(channel.interval_seconds.recip(), "Hz"),
                );
                facts.add(&format!("{} · samples", channel.name), channel.sample_count);
                if peak.valid() {
                    let unit = &channel.unit;
                    facts.add(
                        &format!("{} · observed extrema", channel.name),
                        format!(
                            "{} to {}",
                            display_value(peak.min as f64, unit),
                            display_value(peak.max as f64, unit)
                        ),
                    );
                }
            }
        }
        if let Some(rf) = &data.touchstone {
            let (metadata, band, complete) = rf.measurement();
            facts.add("Ports", metadata.ports);
            facts.add(
                "Observed frequency band",
                format!(
                    "{} to {}",
                    display_value(band[0], "Hz"),
                    display_value(band[1], "Hz")
                ),
            );
            facts.coverage(complete);
        }
        if let Some(logic) = &data.logic {
            facts.add("Channels", logic.lanes.len());
            for (index, lane) in logic.lanes.iter().enumerate() {
                let rate = lane
                    .sample_rate
                    .map(|rate| display_value(rate, "Hz"))
                    .unwrap_or_else(|| "Time-stamped transitions".into());
                facts.add(
                    &format!("Channel {}", index + 1),
                    format!("{} · {rate}", lane.name),
                );
            }
        }
        if let Some(packets) = &data.packets {
            let capture = packets.summary();
            facts.coverage(capture.complete);
            for interface in &capture.interfaces {
                facts.add(
                    &format!(
                        "Interface {}.{}",
                        interface.id.section + 1,
                        interface.id.ordinal
                    ),
                    format!(
                        "link {} · snaplen {} · {}",
                        interface.link_type,
                        interface.snap_len,
                        interface.name.as_deref().unwrap_or("—")
                    ),
                );
            }
            let mut counts = BTreeMap::new();
            for row in &capture.packets {
                *counts
                    .entry(row.protocol.protocol.label())
                    .or_insert(0usize) += 1;
            }
            for (protocol, count) in counts {
                facts.add(&format!("Observed {protocol} packets"), count);
            }
        }
        if let Some(can) = &data.can_capture {
            facts.coverage(can.summary.complete);
            for channel in &can.summary.channels {
                facts.add(
                    &format!("Bus {}", channel.channel),
                    format!(
                        "{} frames · {} errors",
                        channel.frames, channel.error_frames
                    ),
                );
            }
        }
        if let Some(capture) = &data.pulses {
            facts.coverage(capture.complete);
            for (index, signal) in capture.signals.iter().enumerate() {
                let name = format!("Signal {:04}", index + 1);
                facts.add(&name, signal.label());
                for (key, value) in &signal.fields {
                    facts.add(&format!("{name} · {key}"), value);
                }
                if !signal.pulses.is_empty() {
                    facts.add(&format!("{name} · pulse count"), signal.pulses.len());
                    facts.add(
                        &format!("{name} · pulse duration (µs)"),
                        signal.duration_us(),
                    );
                }
            }
        }
        if let Some(tag) = &data.tags {
            facts.tag(tag.document());
        }
        if data.radio.is_some()
            && let Ok(plan) = radio::read(path, cancel)
        {
            for (field, value) in &plan.fields {
                facts.add(field, value);
            }
            for (band, count) in BANDS.into_iter().zip(plan.band_counts()) {
                facts.add(&format!("{band} channels"), count);
            }
            for channel in &plan.channels {
                let name = format!("Channel {:05}", channel.number);
                facts.add(
                    &name,
                    format!(
                        "{} · {} · {} · {}",
                        channel.name,
                        radio::frequency_label(channel.receive_hz),
                        channel.mode,
                        channel
                            .transmit_hz()
                            .map_or_else(|| "No TX".to_owned(), radio::frequency_label)
                    ),
                );
                for (key, value) in &channel.fields {
                    facts.add(&format!("{name} · {key}"), value);
                }
            }
        }
        if let Some(flow) = &data.flowgraph {
            facts.coverage(flow.data.complete);
            let graph = &flow.data.graph;
            for block in &graph.blocks {
                facts.add(&format!("Block {}", block.name), &block.kind);
                for setting in &block.settings {
                    facts.add(
                        &format!("{} · {}", block.name, setting.name),
                        &setting.value,
                    );
                }
            }
            for connection in &graph.connections {
                let (from, to) = (&connection.source, &connection.sink);
                facts.add(
                    &format!(
                        "Connection {}.{} → {}.{}",
                        graph.blocks[from.block].name,
                        from.port,
                        graph.blocks[to.block].name,
                        to.port
                    ),
                    "Present",
                );
            }
        }
        if let Some(iq) = &data.iq {
            facts.add(
                "Complex samples",
                iq.power.samples.end - iq.power.samples.start,
            );
            if let Some(rate) = iq.rate {
                facts.add("Sample rate", display_value(rate, "Hz"));
            }
            if let Some(center) = iq.center_hz {
                facts.add("Center frequency", display_value(center, "Hz"));
            }
        }
        facts
    }

    fn tag(&mut self, document: &Document) {
        if let Document::Memory(memory) = document {
            self.add("Protocol", &memory.protocol);
            for (field, value) in &memory.fields {
                self.add(field, value);
            }
            for row in &memory.rows {
                let mut bytes = String::new();
                for byte in &row.bytes {
                    if let Some(byte) = byte {
                        let _ = write!(bytes, "{byte:02X} ");
                    } else {
                        bytes.push_str("·· ");
                    }
                }
                self.add(
                    &format!("{} · 0x{:04X}", row.name, row.address),
                    bytes.trim_end(),
                );
            }
        }
    }

    fn coverage(&mut self, complete: bool) {
        self.add(
            "Decoded properties",
            if complete {
                "Available records"
            } else {
                "Initial preview records"
            },
        );
    }

    fn add(&mut self, key: &str, value: impl Display) {
        if self.0.len() >= 16_384 || self.1 >= 4 * 1024 * 1024 {
            self.0.insert(
                "Property limit".into(),
                "This snapshot shows a bounded subset".into(),
            );
            return;
        }
        let value = value.to_string();
        let key = &key[..key.floor_char_boundary(key.len().min(512))];
        let value = &value[..value.floor_char_boundary(value.len().min(4096))];
        self.1 += key.len() + value.len();
        self.0.insert(key.to_owned(), value.to_owned());
    }
}
