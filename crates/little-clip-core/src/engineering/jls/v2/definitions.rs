use std::collections::BTreeMap;

use super::{
    Channel, FSR_HEAD, LEVELS, Reader, SIGNAL_DEF, SOURCE_DEF, Signal, UTC_HEAD,
    raw::{Chunk, check_header},
    u64_at,
};

const MAX_METADATA_CHUNKS: usize = 4096;
const MAX_METADATA_PAYLOAD: usize = 16384;

pub(super) struct OpenedSignals {
    sources: BTreeMap<u16, String>,
    pub(super) signals: Vec<Signal>,
}

#[derive(Default)]
struct DefinitionHeads {
    sources: u64,
    signals: u64,
}

#[derive(Default)]
struct SignalDeclaration {
    signal: Option<Signal>,
    fixed_rate: Option<[u64; LEVELS]>,
    utc: Option<u64>,
}

impl OpenedSignals {
    pub(super) fn read(reader: &Reader) -> Result<Self, String> {
        check_header(reader)?;
        let heads = DefinitionHeads::find(reader)?;
        let declarations = signal_declarations(reader, heads.signals)?;
        let sources = source_labels(reader, heads.sources)?;

        let mut signals = Vec::new();
        for mut signal in declarations
            .into_values()
            .filter_map(SignalDeclaration::into_signal)
        {
            signal.find_extent(reader)?;
            if signal.channel.sample_count > i64::MAX as u64 {
                return Err("Joulescope waveform exceeds its sample count limit".into());
            }
            signals.push(signal);
        }
        if signals.is_empty() {
            return Err("Joulescope recording has no supported fixed-rate signal data".into());
        }
        Ok(Self { sources, signals })
    }

    pub(super) fn model(&self) -> String {
        match self.sources.len() {
            0 | 1 => self
                .sources
                .values()
                .find(|value| !value.is_empty())
                .cloned()
                .unwrap_or_else(|| "Joulescope".into()),
            count => format!("Joulescope · {count} sources"),
        }
    }

    pub(super) fn channels(&self, reader: &Reader) -> Vec<Channel> {
        let first_times: Vec<_> = self
            .signals
            .iter()
            .map(|signal| signal.first_utc(reader))
            .collect();
        let origin = first_times.iter().copied().flatten().min_by(f64::total_cmp);

        let mut name_counts = BTreeMap::new();
        for signal in &self.signals {
            *name_counts.entry(signal.channel.name.as_str()).or_insert(0) += 1;
        }
        self.signals
            .iter()
            .zip(first_times)
            .map(|(signal, time)| {
                let mut channel = signal.channel.clone();
                if let Some((origin, time)) = origin.zip(time) {
                    channel.start_seconds = time - origin;
                }
                if name_counts[signal.channel.name.as_str()] > 1
                    && let Some(source) = self.sources.get(&signal.source)
                {
                    channel.name = format!("{source} · {}", channel.name);
                }
                channel
            })
            .collect()
    }
}

impl DefinitionHeads {
    fn find(reader: &Reader) -> Result<Self, String> {
        // Each first definition anchors a linked list that can continue across
        // the data chunks when definitions are added during a recording.
        let mut heads = Self::default();
        let mut offset = 32;
        for _ in 0..MAX_METADATA_CHUNKS {
            if offset == reader.len {
                break;
            }
            let chunk = Chunk::read(reader, offset)?;
            match chunk.tag {
                SOURCE_DEF if heads.sources == 0 => heads.sources = offset,
                SIGNAL_DEF if heads.signals == 0 => heads.signals = offset,
                _ => {}
            }
            if heads.sources != 0 && heads.signals != 0 {
                break;
            }
            offset = chunk.end;
        }
        Ok(heads)
    }
}

impl SignalDeclaration {
    fn into_signal(self) -> Option<Signal> {
        let mut signal = self.signal?;
        let offsets = self.fixed_rate?;
        if offsets[0] == 0 {
            return None;
        }
        signal.offsets = offsets;
        signal.utc = self.utc.unwrap_or(0);
        Some(signal)
    }
}

fn signal_declarations(
    reader: &Reader,
    head: u64,
) -> Result<BTreeMap<u16, SignalDeclaration>, String> {
    let mut declarations = BTreeMap::<u16, SignalDeclaration>::new();
    walk_definitions(reader, head, |chunk| {
        if !(1..256).contains(&chunk.meta) {
            return Ok(());
        }
        match chunk.tag {
            SIGNAL_DEF => {
                let payload = chunk.payload(reader, MAX_METADATA_PAYLOAD)?;
                if let Some(signal) = Signal::definition(chunk.meta, &payload)? {
                    declarations
                        .entry(chunk.meta)
                        .or_default()
                        .signal
                        .get_or_insert(signal);
                }
            }
            FSR_HEAD | UTC_HEAD => {
                let payload = chunk.payload(reader, LEVELS * 8)?;
                if payload.len() != LEVELS * 8 {
                    return Err("Invalid Joulescope track head".into());
                }
                let declaration = declarations.entry(chunk.meta).or_default();
                if chunk.tag == FSR_HEAD {
                    declaration.fixed_rate.get_or_insert_with(|| {
                        std::array::from_fn(|index| u64_at(&payload, index * 8))
                    });
                } else {
                    declaration.utc.get_or_insert(u64_at(&payload, 0));
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(declarations)
}

fn source_labels(reader: &Reader, head: u64) -> Result<BTreeMap<u16, String>, String> {
    let mut sources = BTreeMap::new();
    walk_definitions(reader, head, |chunk| {
        if chunk.tag != SOURCE_DEF {
            return Err("Invalid Joulescope source definition".into());
        }
        if (1..256).contains(&chunk.meta) {
            let payload = chunk.payload(reader, MAX_METADATA_PAYLOAD)?;
            let [name, _, model] = strings::<3>(&payload, 64)?;
            sources
                .entry(chunk.meta)
                .or_insert(if model.is_empty() { name } else { model });
        }
        Ok(())
    })?;
    Ok(sources)
}

fn walk_definitions(
    reader: &Reader,
    mut offset: u64,
    mut visit: impl FnMut(Chunk) -> Result<(), String>,
) -> Result<(), String> {
    for _ in 0..MAX_METADATA_CHUNKS {
        if offset == 0 {
            return Ok(());
        }
        let chunk = Chunk::read(reader, offset)?;
        visit(chunk)?;
        offset = chunk.forward()?;
    }
    Err("Joulescope definition chain exceeds its limit".into())
}

pub(super) fn strings<const N: usize>(
    payload: &[u8],
    offset: usize,
) -> Result<[String; N], String> {
    let mut bytes = payload
        .get(offset..)
        .ok_or("Truncated Joulescope definition")?;
    let mut output = std::array::from_fn(|_| String::new());
    for name in &mut output {
        let nul = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("Unterminated Joulescope label")?;
        *name = String::from_utf8_lossy(&bytes[..nul]).into_owned();
        bytes = &bytes[nul + 1..];
        if bytes.first() == Some(&0x1f) {
            bytes = &bytes[1..];
        }
    }
    Ok(output)
}
