//! Passive ASS/SSA event rows. Fonts, effects, attachments and override tags are never executed.
use super::{Cue, Format, MAX_CUES, timestamp};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};
const STANDARD: &str = "Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text";
const SSA_STANDARD: &str =
    "Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text";
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Other,
    Info,
    Events,
}

pub(super) fn parse(
    source: &str,
    format: Format,
    cancelled: &AtomicBool,
) -> Result<Vec<Cue>, String> {
    let (standard, version) = if format == Format::SubStationAlpha {
        (SSA_STANDARD, "v4.00")
    } else {
        (STANDARD, "v4.00+")
    };
    let mut section = Section::Other;
    let mut events_seen = false;
    let mut columns = Columns::parse(standard, format)?;
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
                columns = Columns::parse(standard, format)?;
            }
            continue;
        }
        let Some((key, value)) = text.split_once(':') else {
            continue;
        };
        match section {
            Section::Info => validate_version(key.trim(), value.trim(), version)?,
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
fn validate_version(key: &str, value: &str, version: &str) -> Result<(), String> {
    if key.eq_ignore_ascii_case("ScriptType") && !value.eq_ignore_ascii_case(version) {
        return Err(format!("This subtitle format expects a {version} script"));
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
        *columns = Columns::parse(value, columns.format)?;
    } else if key.eq_ignore_ascii_case("Dialogue") {
        if cues.len() >= MAX_CUES {
            return Err("Subtitle preview is limited to 100,000 cues".into());
        }
        cues.push(columns.cue(value, line)?);
    }
    Ok(())
}
struct Columns {
    format: Format,
    names: Vec<String>,
    start: usize,
    end: usize,
    speaker: Option<usize>,
}
impl Columns {
    fn parse(value: &str, format: Format) -> Result<Self, String> {
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
            format,
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
        let start_ms = timestamp(fields[self.start].trim(), self.format)?;
        let end_ms = timestamp(fields[self.end].trim(), self.format)?;
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
                            | "marked"
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
    fn ssa_uses_legacy_marks_and_version_without_turning_comments_or_fonts_into_cues() {
        let source = "\u{feff}[Script Info]\r\nScriptType: v4.00\r\n[V4 Styles]\r\nStyle: Default,Sans,24\r\n[Events]\r\nDialogue: Marked=0,0:00:01.25,0:00:03.00,Default,Guide,0,0,0,,{\\b1}Hello, garden!{\\b0}\\NNext line\r\nComment: Marked=0,0:00:00.00,0:01:00.00,Default,Hidden,0,0,0,,Not active\r\n[Fonts]\r\nfontname: /proc/self/status\r\nDialogue: never,interpret,this\r\n";
        let document =
            Document::parse(source, Format::SubStationAlpha, &AtomicBool::new(false)).unwrap();
        assert_eq!(document.cues.len(), 1);
        let cue = &document.cues[0];
        assert_eq!((cue.start_ms, cue.end_ms), (1250, 3000));
        assert_eq!(cue.source_line, 6);
        assert_eq!(cue.identifier, "Guide");
        assert_eq!(cue.text, "{\\b1}Hello, garden!{\\b0}\\NNext line");
        assert!(cue.settings.contains("marked: Marked=0"));
        assert!(!cue.settings.contains("layer:"));
        assert_eq!(document.timeline.duration_ms, 3000);
        assert!(parse(source).is_err()); // Extension-specific version checking remains explicit.
        for invalid in ["v3.00", "v4.00+"] {
            assert!(
                Document::parse(
                    &source.replace("v4.00", invalid),
                    Format::SubStationAlpha,
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
        assert_eq!(
            Format::from_path(std::path::Path::new("captions.SsA")),
            Some(Format::SubStationAlpha)
        );
    }
    #[test]
    fn ssa_custom_actor_fields_commas_and_section_resets_share_the_event_reader() {
        let source = "[Events]\nFormat: Actor,End,Start,Text\nDialogue: Narrator,0:00:03.00,0:00:01.00,  A, comma  \n[Events]\nDialogue: Marked=0,0:00:02.00,0:00:04.00,Default,Second,0,0,0,,Overlap";
        let document =
            Document::parse(source, Format::SubStationAlpha, &AtomicBool::new(false)).unwrap();
        assert_eq!(document.cues[0].identifier, "Narrator");
        assert_eq!(document.cues[0].text, "  A, comma  ");
        assert_eq!(document.cues[1].identifier, "Second");
        assert!(document.cues[1].settings.contains("marked: Marked=0"));
        assert_eq!(document.timeline.peak_concurrency, 2);
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
