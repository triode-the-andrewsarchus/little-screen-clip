//! Literal SFZ mapping and inheritance. Preprocessor expansion belongs to the guarded caller.
use super::{Document, Instrument, Region};
use std::collections::BTreeMap;
type Fields = BTreeMap<String, String>;
/// Parses literal SFZ regions, retaining opcodes for inspection without executing DSP.
///
/// # Errors
/// Rejects unexpanded preprocessing, invalid mappings and excessive input/metadata.
pub fn parse(bytes: &[u8]) -> Result<Document, String> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("SFZ exceeds 16 MiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "SFZ must be UTF-8 text")?;
    let clean = strip_comments(text)?;
    let mut reader = Reader::default();
    let mut rest = clean.as_str();
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        if rest.starts_with('<') {
            let end = rest.find('>').ok_or("Unclosed SFZ header")?;
            reader.header(&rest[1..end])?;
            rest = &rest[end + 1..];
        } else {
            let (key, value, tail) = opcode(rest)?;
            reader.field(key, value)?;
            rest = tail;
        }
    }
    reader.finish()?;
    let mut notes=vec!["Mapping inspector and raw sample audition; envelopes, filters, keyswitches, round robin and other DSP/trigger conditions are not synthesized. Inspect the retained opcodes for those conditions.".into()];
    if reader.ignored {
        notes.push("Non-mapping SFZ sections are not interpreted.".into());
    }
    let doc = Document {
        instruments: vec![Instrument {
            name: "SFZ instrument".into(),
            regions: reader.regions,
        }],
        notes,
        pcm: None,
    };
    doc.validate()?;
    Ok(doc)
}
#[derive(Default)]
struct Reader {
    global: Fields,
    master: Fields,
    group: Fields,
    region: Fields,
    section: String,
    default_path: String,
    regions: Vec<Region>,
    retained: usize,
    ignored: bool,
}
impl Reader {
    fn header(&mut self, name: &str) -> Result<(), String> {
        self.finish()?;
        match name {
            "global" => {
                self.global.clear();
                self.master.clear();
                self.group.clear();
            }
            "master" => {
                self.master.clear();
                self.group.clear();
            }
            "group" => self.group.clear(),
            "region" => self.region.clear(),
            "control" => {}
            _ => self.ignored = true,
        }
        self.section = name.into();
        Ok(())
    }
    fn field(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key.len() > 128 || value.len() > 4096 {
            return Err("SFZ opcode exceeds metadata limits".into());
        }
        if value.contains('$') {
            return Err("Unexpanded SFZ variable".into());
        }
        if self.section == "control" {
            if key == "default_path" {
                self.default_path = value.replace('\\', "/");
            }
            return Ok(());
        }
        let fields = match self.section.as_str() {
            "global" => &mut self.global,
            "master" => &mut self.master,
            "group" => &mut self.group,
            "region" => &mut self.region,
            _ => return Ok(()),
        };
        if fields.len() >= 128 {
            return Err("More than 128 opcodes in an SFZ section".into());
        }
        if key == "key" {
            let note = note(value)?.to_string();
            for key in ["lokey", "hikey", "pitch_keycenter"] {
                fields.insert(key.into(), note.clone());
            }
        } else {
            fields.insert(key.into(), value.into());
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        if self.section != "region" {
            return Ok(());
        }
        if self.regions.len() >= 8192 {
            return Err("More than 8,192 SFZ regions".into());
        }
        let mut fields = self.global.clone();
        fields.extend(self.master.clone());
        fields.extend(self.group.clone());
        fields.extend(self.region.clone());
        let mut region = region(&fields)?;
        if !self.default_path.is_empty() && !region.sample.starts_with(['/', '*']) {
            region.sample = format!(
                "{}{}{}",
                self.default_path,
                if self.default_path.ends_with('/') {
                    ""
                } else {
                    "/"
                },
                region.sample
            );
        }
        self.retained += super::metadata_bytes(&region);
        if self.retained > 16 * 1024 * 1024 {
            return Err("SFZ inherited metadata exceeds the 16 MiB display budget".into());
        }
        self.regions.push(region);
        self.section.clear();
        Ok(())
    }
}
fn region(fields: &Fields) -> Result<Region, String> {
    let sample = fields
        .get("sample")
        .ok_or("SFZ region has no sample")?
        .replace('\\', "/");
    let key = [
        field_note(fields, "lokey", 0)?,
        field_note(fields, "hikey", 127)?,
    ];
    let velocity = [
        field_note(fields, "lovel", 0)?,
        field_note(fields, "hivel", 127)?,
    ];
    let root = field_note(fields, "pitch_keycenter", 60)?;
    let start = integer(fields, "offset")?.unwrap_or(0);
    let end = integer(fields, "end")?
        .map(|n| n.checked_add(1).ok_or("SFZ sample end overflow"))
        .transpose()?;
    let loops = match (integer(fields, "loop_start")?, integer(fields, "loop_end")?) {
        (Some(a), Some(b)) => vec![[a, b.checked_add(1).ok_or("Loop end overflow")?]],
        _ => Vec::new(),
    };
    Ok(Region {
        name: fields
            .get("region_label")
            .cloned()
            .unwrap_or_else(|| sample.clone()),
        sample,
        key,
        velocity,
        root,
        start,
        end,
        loops,
        properties: fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        embedded: None,
    })
}
fn field_note(fields: &Fields, key: &str, default: u8) -> Result<u8, String> {
    fields.get(key).map_or(Ok(default), |v| note(v))
}
fn integer(fields: &Fields, key: &str) -> Result<Option<u64>, String> {
    fields
        .get(key)
        .map(|v| v.parse().map_err(|_| format!("Invalid SFZ {key}")))
        .transpose()
}
fn note(value: &str) -> Result<u8, String> {
    if let Ok(n) = value.parse::<u8>() {
        return (n <= 127)
            .then_some(n)
            .ok_or_else(|| "MIDI value exceeds 127".into());
    }
    let bytes = value.as_bytes();
    let first = bytes.first().copied().unwrap_or(0).to_ascii_uppercase();
    let base = match first {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return Err("Invalid SFZ note/value".into()),
    };
    let (accidental, at) = match bytes.get(1) {
        Some(b'#') => (1, 2),
        Some(b'b') => (-1, 2),
        _ => (0, 1),
    };
    let octave = value
        .get(at..)
        .and_then(|s| s.parse::<i32>().ok())
        .ok_or("Invalid note octave")?;
    let n = (i64::from(octave) + 1) * 12 + base + accidental;
    u8::try_from(n)
        .ok()
        .filter(|n| *n <= 127)
        .ok_or_else(|| "Note is outside MIDI range".into())
}
/// Removes SFZ comments while retaining quoted paths and line boundaries.
///
/// # Errors
/// Rejects unterminated quoted values or block comments.
pub fn strip_comments(text: &str) -> Result<String, String> {
    if text.len() > 16 * 1024 * 1024 {
        return Err("SFZ exceeds 16 MiB".into());
    }
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if c == '"' {
            quoted = !quoted;
            output.push(c);
        } else if !quoted && c == '/' && chars.peek() == Some(&'/') {
            for c in chars.by_ref() {
                if c == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else if !quoted && c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
                if c == '\n' {
                    output.push('\n');
                }
            }
            if !closed {
                return Err("Unclosed SFZ comment".into());
            }
            output.push(' ');
        } else {
            output.push(c);
        }
    }
    if quoted {
        return Err("Unclosed quoted SFZ value".into());
    }
    Ok(output)
}
fn opcode(text: &str) -> Result<(&str, &str, &str), String> {
    if text.starts_with('#') {
        return Err("SFZ preprocessing must be expanded by the guarded loader".into());
    }
    let equal = text.find('=').ok_or("Expected SFZ opcode=value")?;
    let key = text[..equal].trim();
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("Invalid SFZ opcode".into());
    }
    let value = text[equal + 1..].trim_start();
    if let Some(quoted) = value.strip_prefix('"') {
        let end = quoted.find('"').ok_or("Unclosed quoted SFZ value")?;
        return Ok((key, &quoted[..end], &quoted[end + 1..]));
    }
    let mut end = value.len();
    for (at, c) in value.char_indices() {
        if c == '<' || c == '\n' || c == '\r' || (c.is_whitespace() && next_opcode(&value[at..])) {
            end = at;
            break;
        }
    }
    Ok((key, value[..end].trim_end(), &value[end..]))
}
fn next_opcode(value: &str) -> bool {
    let next = value.trim_start();
    let key = next
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    next.as_bytes().get(key) == Some(&b'=')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_ranges_labels_quoted_paths_and_exclusive_loop_bounds() {
        let doc = parse(
            br#"/* mapping */ <control> default_path=samples/
<global> pitch_keycenter=C4 ampeg_release=0.5
<master> lovel=3
<group> lokey=C3 hikey=B4 hivel=63
<region> sample="Soft piano.wav" offset=20 end=99 loop_start=30 loop_end=80 region_label=Quiet keys
<group> key=G2
<region> sample=loud.wav hivel=127 // inherited master
"#,
        )
        .unwrap();
        let regions = &doc.instruments[0].regions;
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].sample, "samples/Soft piano.wav");
        assert_eq!(regions[0].name, "Quiet keys");
        assert_eq!(regions[0].key, [48, 71]);
        assert_eq!(regions[0].root, 60);
        assert_eq!(regions[0].velocity, [3, 63]);
        assert_eq!(regions[0].end, Some(100));
        assert_eq!(regions[0].loops, vec![[30, 81]]);
        assert_eq!(regions[1].key, [43, 43]);
        assert_eq!(regions[1].root, 43);
        assert_eq!(regions[1].velocity, [3, 127]);
        assert!(
            regions[1]
                .properties
                .contains(&("ampeg_release".into(), "0.5".into()))
        );
    }
    #[test]
    fn inherited_attributes_cannot_amplify_a_small_source_past_the_display_budget() {
        let source = format!(
            "<global> description={}\n{}",
            "x".repeat(4096),
            "<region> sample=x\n".repeat(8192)
        );
        assert!(
            parse(source.as_bytes())
                .err()
                .unwrap()
                .contains("16 MiB display budget")
        );
    }
    #[test]
    fn invalid_or_unexpanded_sfz_is_rejected() {
        for text in [
            "<region> sample=x lokey=72 hikey=60",
            "<region> sample=x hivel=128",
            "<region> sample=x pitch_keycenter=C999999999",
            "<region> sample=$MISSING",
            "#include \"x\"",
            "/* unclosed",
            "<region> sample=\"unclosed",
            "<region> key=60",
        ] {
            assert!(parse(text.as_bytes()).is_err(), "{text}");
        }
        assert!(parse(&[0xff]).is_err());
        let source = "<region> sample=x\n".repeat(8193);
        assert!(parse(source.as_bytes()).is_err());
    }
}
