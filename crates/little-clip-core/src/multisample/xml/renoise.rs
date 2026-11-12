//! Original mapping reader for Renoise Instrument.xml and indexed archive samples.
use super::{Regions, child, document, frame, midi};
use crate::multisample::{Document, Instrument, Region};
use roxmltree::Node;

/// Reads an XRNI manifest against its positively identified sample-member inventory.
///
/// # Errors
/// Rejects malformed mappings, missing/duplicate audio indices, unsupported slice
/// associations and metadata budgets. External FileName hints are never opened.
pub fn parse(
    bytes: &[u8],
    fallback: &str,
    members: &[(usize, String)],
) -> Result<Document, String> {
    let xml = document(bytes)?;
    let root = xml.root_element();
    if !root.has_tag_name("RenoiseInstrument") {
        return Err("Archive has no Renoise instrument manifest".into());
    }
    let regions = regions(root, members)?;
    let doc = Document {
        instruments: vec![Instrument {
            name: field(root, "Name")?.unwrap_or(fallback).to_owned(),
            regions,
        }],
        notes: vec!["Renoise source mappings and slice intervals. Audition plays raw source audio; modulation, phrases, effects and pitch processing are not rendered.".into()],
        pcm: None,
    };
    doc.validate()?;
    Ok(doc)
}
fn regions(root: Node<'_, '_>, members: &[(usize, String)]) -> Result<Vec<Region>, String> {
    let generator = child(root, "SampleGenerator")?.ok_or("Missing Renoise sample generator")?;
    let samples = child(generator, "Samples")?.ok_or("Missing Renoise samples")?;
    let mut regions = Regions::new();
    let mut primary: Option<(String, Vec<u64>)> = None;
    let mut alias_index = 0;
    for (index, sample) in samples
        .children()
        .filter(|n| n.has_tag_name("Sample"))
        .enumerate()
    {
        let mut region = mapping(sample)?;
        if field(sample, "IsAlias")? == Some("true") {
            apply_alias(&mut region, sample, primary.as_ref(), alias_index)?;
            alias_index += 1;
        } else {
            region.sample = member(members, index)?.to_owned();
            primary = Some((region.sample.clone(), markers(sample)?));
            alias_index = 0;
        }
        regions.push(region)?;
    }
    Ok(regions.values)
}
fn apply_alias(
    region: &mut Region,
    sample: Node<'_, '_>,
    primary: Option<&(String, Vec<u64>)>,
    alias_index: usize,
) -> Result<(), String> {
    let (path, markers) = primary
        .as_ref()
        .ok_or("Slice alias has no primary sample")?;
    let start = *markers
        .get(alias_index)
        .ok_or("Slice alias has no matching marker")?;
    let length = frame(field(sample, "LoopEnd")?)?.ok_or("Slice alias has no length")?;
    let end = start.checked_add(length).ok_or("Slice end overflows")?;
    if length == 0 || markers.get(alias_index + 1).is_some_and(|next| end > *next) {
        return Err("Slice alias exceeds its source interval".into());
    }
    region.sample = path.clone();
    region.start = start;
    region.end = Some(end);
    // Alias loop offsets are relative to the slice, not the primary sample.
    region.loops.clear();
    Ok(())
}
fn field<'a>(node: Node<'a, 'a>, name: &str) -> Result<Option<&'a str>, String> {
    Ok(child(node, name)?.and_then(|n| n.text()).map(str::trim))
}
fn member(members: &[(usize, String)], index: usize) -> Result<&str, String> {
    let mut matches = members.iter().filter(|(i, _)| *i == index);
    let first = matches
        .next()
        .ok_or("Renoise sample has no embedded audio member")?;
    if matches.next().is_some() {
        return Err("Renoise sample has duplicate embedded audio members".into());
    }
    Ok(&first.1)
}
fn mapping(sample: Node<'_, '_>) -> Result<Region, String> {
    let map = child(sample, "Mapping")?.ok_or("Renoise sample has no mapping")?;
    let loops = loops(sample)?;
    let properties = properties(sample, map)?;
    Ok(Region {
        name: field(sample, "Name")?.unwrap_or("Sample").to_owned(),
        sample: String::new(),
        key: range(map, "NoteStart", "NoteEnd", 119)?,
        velocity: range(map, "VelocityStart", "VelocityEnd", 127)?,
        root: midi(field(map, "BaseNote")?, 48)?,
        start: 0,
        end: None,
        loops,
        properties,
        embedded: None,
    })
}
fn range(map: Node<'_, '_>, low: &str, high: &str, max: u8) -> Result<[u8; 2], String> {
    Ok([midi(field(map, low)?, 0)?, midi(field(map, high)?, max)?])
}
fn loops(sample: Node<'_, '_>) -> Result<Vec<[u64; 2]>, String> {
    let start = frame(field(sample, "LoopStart")?)?.unwrap_or(0);
    let end = frame(field(sample, "LoopEnd")?)?.unwrap_or(0);
    match field(sample, "LoopMode")? {
        None | Some("Off") => Ok(Vec::new()),
        Some(_) if end > start => Ok(vec![[start, end]]),
        _ => Err("Invalid Renoise loop interval".into()),
    }
}
fn properties(sample: Node<'_, '_>, map: Node<'_, '_>) -> Result<Vec<(String, String)>, String> {
    let mut properties = Vec::new();
    for node in sample
        .children()
        .chain(map.children())
        .filter(|n| n.is_element())
    {
        if let Some(value) = node
            .text()
            .filter(|_| node.children().all(|c| !c.is_element()))
        {
            if properties.len() >= 128 || value.len() > 4096 || node.tag_name().name().len() > 128 {
                return Err("Renoise sample metadata exceeds the budget".into());
            }
            properties.push((node.tag_name().name().to_owned(), value.to_owned()));
        }
    }
    Ok(properties)
}
fn markers(sample: Node<'_, '_>) -> Result<Vec<u64>, String> {
    let mut markers = Vec::new();
    if let Some(parent) = child(sample, "SliceMarkers")? {
        for node in parent.children().filter(|n| n.has_tag_name("SliceMarker")) {
            let position =
                frame(field(node, "SamplePosition")?)?.ok_or("Missing slice position")?;
            if markers.len() >= 8192 || markers.last().is_some_and(|last| position <= *last) {
                return Err("Invalid or excessive Renoise slice markers".into());
            }
            markers.push(position);
        }
    }
    Ok(markers)
}
