//! Read-only sample mappings, shared by folder strips and the multisample inspector.
pub mod sf2;
pub mod sfz;
pub mod xml;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
/// Encoded PCM location in a retained SoundFont snapshot; never a filesystem reference.
#[derive(Clone, Debug)]
pub struct Pcm16Sample {
    pub bytes: std::ops::Range<usize>,
    pub rate: u32,
}
/// A selected source within an instrument, without opening it during mapping.
#[derive(Clone, Debug)]
pub enum EmbeddedSample {
    /// Mono PCM16 in the document's retained SoundFont bytes.
    Pcm16(Pcm16Sample),
    /// An encoded audio member in a separately retained instrument archive.
    Archive { path: String },
}
/// A source region. Ranges are inclusive MIDI values; sample end is exclusive.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Region {
    pub name: String,
    pub sample: String,
    pub key: [u8; 2],
    pub velocity: [u8; 2],
    pub root: u8,
    #[serde(default)]
    pub start: u64,
    #[serde(default)]
    pub end: Option<u64>,
    #[serde(default)]
    pub loops: Vec<[u64; 2]>,
    #[serde(default)]
    pub properties: Vec<(String, String)>,
    #[serde(skip)]
    pub embedded: Option<EmbeddedSample>,
}
/// One preset or instrument, in source order.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Instrument {
    pub name: String,
    pub regions: Vec<Region>,
}
/// A bounded mapping inventory, retaining PCM only for an open SoundFont inspector.
pub struct Document {
    pub instruments: Vec<Instrument>,
    pub notes: Vec<String>,
    pub pcm: Option<Arc<[u8]>>,
}
impl Document {
    /// Validates helper output and parser results before mapping or audition.
    ///
    /// # Errors
    /// Rejects excessive counts/strings, inverted ranges and invalid MIDI values.
    pub fn validate(&self) -> Result<(), String> {
        if self.instruments.len() > 256 {
            return Err("More than 256 instruments".into());
        }
        let mut count = 0;
        let mut bytes = 0;
        for instrument in &self.instruments {
            if instrument.name.len() > 1024 {
                return Err("Instrument name is too long".into());
            }
            for region in &instrument.regions {
                count += 1;
                if count > 8192 {
                    return Err("More than 8,192 sample regions".into());
                }
                validate_region(region)?;
                bytes += metadata_bytes(region);
                if bytes > 16 * 1024 * 1024 {
                    return Err("Region metadata exceeds the 16 MiB display budget".into());
                }
            }
        }
        Ok(())
    }
}
fn validate_mapping_bounds(region: &Region) -> Result<(), String> {
    if region.end.is_some_and(|end| end < region.start) {
        return Err("Sample region end precedes its start".into());
    }
    if region.key[0] > region.key[1]
        || region.key[1] > 127
        || region.velocity[0] > region.velocity[1]
        || region.velocity[1] > 127
        || region.root > 127
    {
        return Err("Invalid MIDI key/velocity mapping".into());
    }
    Ok(())
}
fn validate_region(region: &Region) -> Result<(), String> {
    validate_mapping_bounds(region)?;
    if region.name.len() > 1024
        || region.sample.len() > 4096
        || region.properties.len() > 128
        || region.loops.len() > 32
    {
        return Err("Region metadata exceeds display limits".into());
    }
    if let Some(EmbeddedSample::Archive { path }) = &region.embedded {
        if path.is_empty() || path.len() > 4096 {
            return Err("Archive member path exceeds display limits".into());
        }
    }
    if region
        .properties
        .iter()
        .any(|(key, value)| key.len() > 128 || value.len() > 4096)
    {
        return Err("Region attribute exceeds display limits".into());
    }
    Ok(())
}

fn metadata_bytes(region: &Region) -> usize {
    region.name.len()
        + region.sample.len()
        + match &region.embedded {
            Some(EmbeddedSample::Archive { path }) => path.len(),
            _ => 0,
        }
        + region
            .properties
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>()
}
