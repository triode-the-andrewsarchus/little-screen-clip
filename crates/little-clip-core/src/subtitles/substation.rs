//! Passive ASS event rows. Fonts, effects, attachments and override tags are never executed.
use super::{Cue, Format, MAX_CUES, timestamp};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};
const STANDARD: &str = "Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text";
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Other,
    Info,
    Events,
}

pub(super) fn parse(source: &str, cancelled: &AtomicBool) -> Result<Vec<Cue>, String> {
    let mut section = Section::Other;
    let mut events_seen = false;
    let mut columns = Columns::parse(STANDARD)?;
    let mut cues = Vec::new();
    for (line, text) in source.lines().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Subtitle loading cancelled".into());
        }
        let text = text.trim_start();
        if let Some(header) = text
            .trim()
            .strip_prefix('[')
            .and_then(|text| text.strip_suffix(']'))
        {
            section = match header.to_ascii_lowercase().as_str() {
                "script info" => Section::Info,
                "events" => Section::Events,
                _ => Section::Other,
            };
            if section == Section::Events {
                events_seen = true;
                columns = Columns::parse(STANDARD)?;
            }
            continue;
        }
        let Some((key, value)) = text.split_once(':') else {
            continue;
        };
        match section {
            Section::Info => validate_version(key.trim(), value.trim())?,
            Section::Events => event(
                &mut columns,
                &mut cues,
                key.trim(),
                value.trim_start(),
                line + 1,
            )
            .map_err(|error| format!("Subtitle source line {}: {error}", line + 1))?,
            Section::Other => {}
        }
    }
    if !events_seen {
        return Err("Missing subtitle [Events] section".into());
    }
    Ok(cues)
}
fn validate_version(key: &str, value: &str) -> Result<(), String> {
    if key.eq_ignore_ascii_case("ScriptType") && !value.eq_ignore_ascii_case("v4.00+") {
        return Err("The ASS viewer expects a v4.00+ script".into());
    }
    Ok(())
}
fn event(
    columns: &mut Columns,
    cues: &mut Vec<Cue>,
    key: &str,
    value: &str,
    line: usize,
) -> Result<(), String> {
    if key.eq_ignore_ascii_case("Format") {
        *columns = Columns::parse(value)?;
    } else if key.eq_ignore_ascii_case("Dialogue") {
        if cues.len() >= MAX_CUES {
            return Err("Subtitle preview is limited to 100,000 cues".into());
        }
        cues.push(columns.cue(value, line)?);
    }
    Ok(())
}
struct Columns {
    names: Vec<String>,
    start: usize,
    end: usize,
    speaker: Option<usize>,
}
impl Columns {
    fn parse(value: &str) -> Result<Self, String> {
        let names: Vec<_> = value
            .split(',')
            .take(65)
            .map(|name| name.trim().to_ascii_lowercase())
            .collect();
        if names.len() > 64 || names.iter().any(|name| name.is_empty() || name.len() > 128) {
            return Err(
                "Invalid event Format fields (maximum 64 fields / 128 bytes per name)".into(),
            );
        }
        let unique: BTreeSet<_> = names.iter().collect();
        if unique.len() != names.len() || names.last().map(String::as_str) != Some("text") {
            return Err("Event Format requires unique fields with Text last".into());
        }
        let index = |field| names.iter().position(|name| name == field);
        let start = index("start").ok_or("Event Format is missing Start")?;
        let end = index("end").ok_or("Event Format is missing End")?;
        let speaker = index("name").or_else(|| index("actor"));
        Ok(Self {
            names,
            start,
            end,
            speaker,
        })
    }
    fn cue(&self, value: &str, source_line: usize) -> Result<Cue, String> {
        let fields: Vec<_> = value.splitn(self.names.len(), ',').collect();
        if fields.len() != self.names.len() {
            return Err("Dialogue does not match its Format field count".into());
        }
        let start_ms = timestamp(fields[self.start].trim(), Format::AdvancedSubStationAlpha)?;
        let end_ms = timestamp(fields[self.end].trim(), Format::AdvancedSubStationAlpha)?;
        if end_ms < start_ms {
            return Err("Cue ends before it starts".into());
        }
        let identifier = self.speaker.map_or("", |index| fields[index].trim());
        if identifier.len() > 4096 {
            return Err("Cue name exceeds 4096 bytes".into());
        }
        Ok(Cue {
            identifier: identifier.to_owned(),
            start_ms,
            end_ms,
            text: fields[fields.len() - 1].to_owned(),
            source_line,
            settings: self.metadata(&fields),
        })
    }
    fn metadata(&self, fields: &[&str]) -> String {
        self.names
            .iter()
            .zip(fields)
            .filter(|(name, value)| {
                !value.trim().is_empty()
                    && matches!(
                        name.as_str(),
                        "layer"
                            | "style"
                            | "name"
                            | "actor"
                            | "marginl"
                            | "marginr"
                            | "marginv"
                            | "effect"
                    )
            })
            .map(|(name, value)| {
                format!(
                    "{name}: {}",
                    value.trim().chars().take(80).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitles::Document;
    fn parse(source: &str) -> Result<Document, String> {
        Document::parse(
            source,
            Format::AdvancedSubStationAlpha,
            &AtomicBool::new(false),
        )
    }
    #[test]
    fn ass_uses_declared_fields_and_preserves_commas_tags_spaces_and_source_lines() {
        let source = "\u{feff}[Script Info]\r\nScriptType: v4.00+\r\nTitle: Garden\r\n[V4+ Styles]\r\nStyle: Default,SomeFont,24\r\n[Events]\r\nFormat: End,Name,Start,Style,Text\r\nDialogue: 0:00:03.00,Guide,0:00:01.25,Default,  {\\i1}Hello, garden!{\\i0}\\NNext line  \r\nComment: 0:99:00.00,Hidden,0:00:00.00,Default,Not active\r\nFormat: Start,End,Text\r\nDialogue: 0:00:02.50,0:00:04.00,Overlap\r\n[Fonts]\r\nfontname: /proc/self/status\r\nDialogue: never,interpret,this\r\n";
        let document = parse(source).unwrap();
        assert_eq!(document.cues.len(), 2);
        let cue = &document.cues[0];
        assert_eq!((cue.start_ms, cue.end_ms), (1250, 3000));
        assert_eq!(cue.identifier, "Guide");
        assert_eq!(cue.source_line, 8);
        assert_eq!(cue.text, "  {\\i1}Hello, garden!{\\i0}\\NNext line  ");
        assert!(cue.settings.contains("style: Default"));
        assert_eq!(document.cues[1].source_line, 11);
        assert_eq!(document.timeline.peak_concurrency, 2);
    }
    #[test]
    fn ass_default_layout_zero_duration_markers_and_section_scopes_are_supported() {
        let source = "[Events]\nFormat: Start,End,Text\nDialogue: 0:00:01.00,0:00:02.00,A\n[Events]\nDialogue: 0,0:00:10.00,0:00:10.00,Default,Marker,0,0,0,,Instant";
        let document = parse(source).unwrap();
        assert_eq!(document.cues.len(), 2);
        assert_eq!(document.cues[1].identifier, "Marker");
        assert_eq!(document.timeline.duration_ms, 10_000);
        assert_eq!(document.timeline.peak_concurrency, 1);
        assert!(parse("[Script Info]\nTitle: No events").is_err());
        assert!(parse("[Events]\n; Empty script").unwrap().cues.is_empty());
    }
    #[test]
    fn ass_invalid_layouts_versions_intervals_and_resource_limits_fail_explicitly() {
        for source in [
            "[Script Info]\nScriptType: v4.00\n[Events]",
            "[Events]\nFormat: Start,End,Text,Name",
            "[Events]\nFormat: Start,START,Text",
            "[Events]\nFormat: Start,Text",
            "[Events]\nFormat: Start,End,Text\nDialogue: 0:00:01.00,Missing text field",
            "[Events]\nFormat: Start,End,Text\nDialogue: 0:00:02.00,0:00:01.00,Backwards",
            "[Events]\nFormat: Start,End,Text\nDialogue: 0:00:01.000,0:00:02.00,Three fraction digits",
            "[Events]\nFormat: Start,End,Text\nDialogue: 0:00:60.00,0:01:01.00,Bad seconds",
            "[Events]\nFormat: Start,End,Text\nDialogue: 9999999999999999999:00:00.00,9999999999999999999:00:01.00,Overflow",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
        assert!(
            parse(&format!(
                "[Events]\nFormat: {},Start,End,Text",
                "Other,".repeat(64)
            ))
            .is_err()
        );
        assert!(
            parse(&format!(
                "[Events]\nDialogue: 0,0:00:01.00,0:00:02.00,Default,{},0,0,0,,Text",
                "x".repeat(4097)
            ))
            .is_err()
        );
        let source = format!(
            "[Events]\nFormat: Start,End,Text\n{}",
            "Dialogue: 0:00:00.00,0:00:00.01,x\n".repeat(MAX_CUES + 1)
        );
        assert!(parse(&source).unwrap_err().contains("100,000"));
        assert!(
            Document::parse(
                "[Events]",
                Format::AdvancedSubStationAlpha,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
