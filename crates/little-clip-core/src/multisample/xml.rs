//! Original read-only mappings from the published DecentSampler and .multisample schemas.
use super::{Document, Instrument, Region};
use roxmltree::Node;

/// Reads a positively identified sampler XML root without following resources.
///
/// # Errors
/// Rejects generic XML, custom entities, malformed mappings and excessive metadata.
pub fn parse(bytes: &[u8], fallback_name: &str) -> Result<Document, String> {
    let xml = document(bytes)?;
    let root = xml.root_element();
    let (regions, format) = match root.tag_name().name() {
        "DecentSampler" => (decent(root)?, "DecentSampler"),
        "multisample" => (bitwig(root)?, "Bitwig/PreSonus multisample"),
        _ => return Err("XML is not a recognized sample mapping".into()),
    };
    let doc = Document {
        instruments: vec![Instrument {
            name: root.attribute("name").unwrap_or(fallback_name).to_owned(),
            regions,
        }],
        notes: vec![format!(
            "{format} mapping preview. Audition plays one source recording; effects, envelopes, pitch processing, round-robin and conditional triggers are not rendered."
        )],
        pcm: None,
    };
    doc.validate()?;
    Ok(doc)
}
mod renoise;
pub use renoise::parse as parse_renoise;
fn document(bytes: &[u8]) -> Result<roxmltree::Document<'_>, String> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Sample mapping XML exceeds 4 MiB".into());
    }
    crate::xml_content::validate(bytes, 100_000, 128)?;
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
        },
    )
    .map_err(|e| e.to_string())
}
struct Regions {
    values: Vec<Region>,
    bytes: usize,
}
impl Regions {
    fn new() -> Self {
        Self {
            values: Vec::new(),
            bytes: 0,
        }
    }
    fn push(&mut self, region: Region) -> Result<(), String> {
        super::validate_region(&region)?;
        self.bytes += super::metadata_bytes(&region);
        if self.values.len() >= 8192 || self.bytes > 16 * 1024 * 1024 {
            return Err("Sample mappings exceed the region/metadata budget".into());
        }
        self.values.push(region);
        Ok(())
    }
}
fn decent(root: Node<'_, '_>) -> Result<Vec<Region>, String> {
    let groups = child(root, "groups")?.ok_or("DecentSampler preset has no groups")?;
    let mut regions = Regions::new();
    for group in groups.children().filter(|n| n.has_tag_name("group")) {
        for sample in group.children().filter(|n| n.has_tag_name("sample")) {
            regions.push(decent_region(sample)?)?;
        }
    }
    Ok(regions.values)
}
fn inherited<'a>(sample: Node<'a, 'a>, key: &str) -> Option<&'a str> {
    sample.ancestors().take(3).find_map(|n| n.attribute(key))
}
fn decent_region(sample: Node<'_, '_>) -> Result<Region, String> {
    let field = |key| inherited(sample, key);
    let path = field("path")
        .ok_or("DecentSampler sample has no path")?
        .replace('\\', "/");
    let region = Region {
        name: sample.attribute("name").unwrap_or(&path).to_owned(),
        sample: path,
        key: [midi(field("loNote"), 0)?, midi(field("hiNote"), 127)?],
        velocity: [midi(field("loVel"), 0)?, midi(field("hiVel"), 127)?],
        root: midi(field("rootNote"), 60)?,
        start: frame(field("start"))?.unwrap_or(0),
        end: inclusive_end(frame(field("end"))?)?,
        loops: decent_loops(sample)?,
        properties: properties(sample, true)?,
        embedded: None,
    };
    Ok(region)
}
fn bitwig(root: Node<'_, '_>) -> Result<Vec<Region>, String> {
    let mut regions = Regions::new();
    for sample in root.children().filter(|n| n.has_tag_name("sample")) {
        regions.push(bitwig_region(sample)?)?;
    }
    Ok(regions.values)
}
fn bitwig_region(sample: Node<'_, '_>) -> Result<Region, String> {
    let key = child(sample, "key")?;
    let velocity = child(sample, "velocity")?;
    let path = sample
        .attribute("file")
        .ok_or("Multisample sample has no file")?
        .replace('\\', "/");
    let mut region = Region {
        name: path.clone(),
        sample: path,
        key: [
            midi(key.and_then(|n| n.attribute("low")), 0)?,
            midi(key.and_then(|n| n.attribute("high")), 127)?,
        ],
        velocity: [
            midi(velocity.and_then(|n| n.attribute("low")), 1)?,
            midi(velocity.and_then(|n| n.attribute("high")), 127)?,
        ],
        root: midi(key.and_then(|n| n.attribute("root")), 60)?,
        start: frame(sample.attribute("sample-start"))?.unwrap_or(0),
        end: frame(sample.attribute("sample-stop"))?,
        loops: bitwig_loops(sample)?,
        properties: properties(sample, false)?,
        embedded: None,
    };
    for node in sample.children().filter(|n| n.is_element()) {
        append_properties(&mut region.properties, node)?;
    }
    Ok(region)
}
fn child<'a>(node: Node<'a, 'a>, name: &str) -> Result<Option<Node<'a, 'a>>, String> {
    let mut nodes = node.children().filter(|n| n.has_tag_name(name));
    let first = nodes.next();
    if nodes.next().is_some() {
        return Err(format!("Duplicate sample mapping {name} element"));
    }
    Ok(first)
}
fn midi(value: Option<&str>, fallback: u8) -> Result<u8, String> {
    match value {
        None => Ok(fallback),
        Some(value) => value
            .parse::<u8>()
            .ok()
            .filter(|v| *v <= 127)
            .ok_or("Invalid MIDI mapping value".into()),
    }
}
fn frame(value: Option<&str>) -> Result<Option<u64>, String> {
    value
        .map(|v| {
            let n = v
                .parse::<f64>()
                .map_err(|_| "Invalid sample frame position")?;
            if !n.is_finite() || n < 0.0 || n >= u64::MAX as f64 {
                return Err("Sample frame position is outside the supported range".into());
            }
            Ok(n.floor() as u64)
        })
        .transpose()
}
fn inclusive_end(value: Option<u64>) -> Result<Option<u64>, String> {
    value
        .map(|n| n.checked_add(1).ok_or("Sample end overflows".into()))
        .transpose()
}
fn properties(sample: Node<'_, '_>, inherit: bool) -> Result<Vec<(String, String)>, String> {
    let mut result = Vec::new();
    for node in sample.ancestors().take(if inherit { 3 } else { 1 }) {
        append_properties(&mut result, node)?;
    }
    Ok(result)
}
fn append_properties(result: &mut Vec<(String, String)>, node: Node<'_, '_>) -> Result<(), String> {
    for attr in node.attributes() {
        if result.len() >= 128
            || node.tag_name().name().len() + 1 + attr.name().len() > 128
            || attr.value().len() > 4096
        {
            return Err("Sample mapping properties exceed the display budget".into());
        }
        result.push((
            format!("{}.{}", node.tag_name().name(), attr.name()),
            attr.value().to_owned(),
        ));
    }
    Ok(())
}

fn decent_loops(sample: Node<'_, '_>) -> Result<Vec<[u64; 2]>, String> {
    let field = |key| inherited(sample, key);
    let mut loops = Vec::new();
    if !matches!(field("loopEnabled"), Some("false" | "0"))
        && let (Some(start), Some(end)) = (
            frame(field("loopStart"))?,
            inclusive_end(frame(field("loopEnd"))?)?,
        )
        && end > start
    {
        loops.push([start, end]);
    }
    Ok(loops)
}

fn bitwig_loops(sample: Node<'_, '_>) -> Result<Vec<[u64; 2]>, String> {
    let mut loops = Vec::new();
    if let Some(node) = child(sample, "loop")?
        && matches!(node.attribute("mode"), Some("loop" | "ping-pong"))
        && let (Some(start), Some(end)) = (
            frame(node.attribute("start"))?,
            frame(node.attribute("stop"))?,
        )
        && end > start
    {
        loops.push([start, end]);
    }
    Ok(loops)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decent_inheritance_velocity_layers_and_inclusive_frames_are_preserved() {
        let doc = parse(br#"<DecentSampler><groups loNote="36" hiNote="72" rootNote="48" volume="0.5"><group name="Keys" loVel="10" hiVel="100" volume="0.4"><sample path="Soft tone.wav" end="22049" loopStart="10" loopEnd="99"/><sample path="Bright.wav" loVel="101" hiVel="127" rootNote="60"/></group></groups></DecentSampler>"#,"Harbor").unwrap();
        let regions = &doc.instruments[0].regions;
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].key, [36, 72]);
        assert_eq!(regions[0].velocity, [10, 100]);
        assert_eq!(regions[0].root, 48);
        assert_eq!(regions[0].end, Some(22050));
        assert_eq!(regions[0].loops, [[10, 100]]);
        assert!(
            regions[0]
                .properties
                .contains(&("groups.volume".into(), "0.5".into()))
        );
        assert!(
            regions[0]
                .properties
                .contains(&("group.volume".into(), "0.4".into()))
        );
        assert_eq!(regions[1].velocity, [101, 127]);
        assert_eq!(regions[1].root, 60);
    }
    #[test]
    fn bitwig_mapping_uses_its_own_frame_and_range_conventions() {
        let doc = parse(br#"<multisample name="Harbor"><group name="Keys"/><sample file="Samples/Bright.wav" sample-start="12.5" sample-stop="22050"><key root="60" low="48" high="71"/><velocity low="64" high="127"/><loop mode="loop" start="20.5" stop="99.8"/></sample></multisample>"#,"fallback").unwrap();
        let region = &doc.instruments[0].regions[0];
        assert_eq!(doc.instruments[0].name, "Harbor");
        assert_eq!(region.key, [48, 71]);
        assert_eq!(region.velocity, [64, 127]);
        assert_eq!(region.start, 12);
        assert_eq!(region.end, Some(22050));
        assert_eq!(region.loops, [[20, 99]]);
    }
    #[test]
    fn unrelated_xml_entities_and_invalid_ranges_do_not_become_sample_maps() {
        for xml in [
            "<preset><sample file='x.wav'/></preset>",
            "<multisample><sample file='x'><key low='99' high='1'/></sample></multisample>",
            "<multisample><sample file='x' sample-stop='NaN'/></multisample>",
            "<multisample><sample file='x' sample-start='100' sample-stop='20'/></multisample>",
            "<multisample><sample file='x'><key/><key/></sample></multisample>",
            "<!DOCTYPE x [<!ENTITY e SYSTEM 'file:///etc/passwd'>]><multisample name='&e;'/>",
        ] {
            assert!(parse(xml.as_bytes(), "x").is_err(), "{xml}");
        }
        let tag = "x".repeat(10000);
        let xml = format!("<multisample><sample file='x'><{tag} a='b'/></sample></multisample>");
        assert!(parse(xml.as_bytes(), "x").is_err());
    }
}
