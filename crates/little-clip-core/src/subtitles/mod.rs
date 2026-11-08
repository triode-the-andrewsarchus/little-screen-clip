//! Passive subtitle cue parsing and bounded timeline summaries.
mod substation;
mod timeline;
#[cfg(test)]
mod webvtt_tests;
use std::{
    borrow::Cow,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
pub use timeline::{Bin, Timeline};

/// Maximum encoded subtitle text accepted by the cue reader.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_CUES: usize = 100_000;

/// The timestamp and block syntax used by a subtitle file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// SubRip counters and comma-separated milliseconds.
    SubRip,
    /// WebVTT headers, identifiers and dot-separated milliseconds.
    WebVtt,
    /// ASS v4+ event rows with centisecond timestamps.
    AdvancedSubStationAlpha,
}
impl Format {
    /// Recognizes a supported filename extension without a leading dot.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "srt" => Some(Self::SubRip),
            "vtt" => Some(Self::WebVtt),
            "ass" => Some(Self::AdvancedSubStationAlpha),
            _ => None,
        }
    }
    /// Recognizes a subtitle format from its path extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()?.to_str().and_then(Self::from_extension)
    }
}

/// One subtitle cue, in source-file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    /// Original counter, identifier or dialogue speaker name, when present.
    pub identifier: String,
    /// Inclusive start time, in milliseconds from the media origin.
    pub start_ms: u64,
    /// Exclusive end time; zero-duration cues remain inspectable markers.
    pub end_ms: u64,
    /// Caption lines joined with line feeds, with formatting text preserved.
    pub text: String,
    /// Positioning or event metadata for passive inspection.
    pub settings: String,
    /// One-based source line containing the timestamp.
    pub source_line: usize,
}

/// A subtitle document and its full-duration cue-density timeline.
#[derive(Clone, Debug)]
pub struct Document {
    /// Cues in their original source order.
    pub cues: Vec<Cue>,
    /// Bounded timeline bins and exact peak concurrency.
    pub timeline: Timeline,
}
impl Document {
    /// Parses UTF-8 SubRip text without executing or rendering embedded markup.
    ///
    /// Accepts CRLF/LF, a UTF-8 BOM, optional counters and overlapping cues.
    ///
    /// # Errors
    /// Returns an error for invalid timings, resource limits or cancellation.
    ///
    /// # Examples
    /// ```
    /// use little_clip_core::subtitles::Document;
    /// use std::sync::atomic::AtomicBool;
    /// let source = "1\n00:00:01,000 --> 00:00:02,500\nHello\n";
    /// let doc = Document::parse_srt(source, &AtomicBool::new(false))?;
    /// assert_eq!(doc.cues[0].text, "Hello");
    /// assert_eq!(doc.timeline.duration_ms, 2500);
    /// # Ok::<(), String>(())
    /// ```
    pub fn parse_srt(source: &str, cancelled: &AtomicBool) -> Result<Self, String> {
        Self::parse(source, Format::SubRip, cancelled)
    }

    /// Parses subtitle cues while preserving caption markup and positioning text.
    ///
    /// WebVTT comment, style and region blocks stay outside the cue model; callers
    /// retain the original source for viewing and editing those blocks. Styles
    /// are never applied. Accepts LF, CRLF and CR line endings and an optional BOM.
    /// The timeline uses cue timestamps without applying external clock mappings.
    /// ASS uses dialogue events only; comments, styles, fonts and attachments remain
    /// in the caller's original source. Event metadata summaries are bounded and
    /// override tags remain literal text. UTF-8 source editing preserves them.
    ///
    /// # Errors
    /// Returns an error for invalid headers/event layouts, invalid cue timings,
    /// identifiers over 4096 bytes, resource limits or cancellation.
    ///
    /// # Examples
    /// ```
    /// use little_clip_core::subtitles::{Document, Format};
    /// use std::sync::atomic::AtomicBool;
    /// let source = "WEBVTT\n\nopening\n00:01.000 --> 00:03.500 align:start\nGarden notes\n";
    /// let doc = Document::parse(source, Format::WebVtt, &AtomicBool::new(false))?;
    /// assert_eq!(doc.cues[0].identifier, "opening");
    /// assert_eq!(doc.cues[0].settings, "align:start");
    /// assert_eq!(doc.timeline.duration_ms, 3500);
    /// # Ok::<(), String>(())
    /// ```
    pub fn parse(source: &str, format: Format, cancelled: &AtomicBool) -> Result<Self, String> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Subtitle loading cancelled".into());
        }
        if source.len() > MAX_BYTES {
            return Err("Subtitle preview is limited to 8 MiB".into());
        }
        let normalized = if source.contains('\r') {
            Cow::Owned(source.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(source)
        };
        let source = normalized.trim_start_matches('\u{feff}');
        let cues = match format {
            Format::AdvancedSubStationAlpha => substation::parse(source, cancelled)?,
            _ => parse_blocks(source, format, cancelled)?,
        };
        let timeline = Timeline::build(&cues, cancelled)?;
        Ok(Self { cues, timeline })
    }
}
fn parse_blocks(source: &str, format: Format, cancelled: &AtomicBool) -> Result<Vec<Cue>, String> {
    let mut lines = source.lines().enumerate();
    if format == Format::WebVtt {
        read_vtt_header(&mut lines)?;
    }
    let mut cues = Vec::new();
    let mut block = Vec::new();
    for (line, text) in lines {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Subtitle loading cancelled".into());
        }
        if text.trim().is_empty() {
            append_block(&mut cues, &mut block, format)?;
        } else {
            block.push((line + 1, text));
        }
    }
    append_block(&mut cues, &mut block, format)?;
    Ok(cues)
}
fn read_vtt_header<'a>(lines: &mut impl Iterator<Item = (usize, &'a str)>) -> Result<(), String> {
    let (_, header) = lines.next().ok_or("Missing WEBVTT header")?;
    let suffix = header
        .strip_prefix("WEBVTT")
        .ok_or("Missing WEBVTT header")?;
    if !suffix.is_empty() && !suffix.starts_with([' ', '\t']) {
        return Err("Invalid WEBVTT header".into());
    }
    if header.contains("-->") {
        return Err("Invalid WEBVTT header".into());
    }
    for (_, line) in lines.by_ref() {
        if line.trim().is_empty() {
            return Ok(());
        }
        if line.contains("-->") {
            return Err("A blank line must separate the WEBVTT header and cues".into());
        }
    }
    Ok(())
}
fn vtt_metadata(line: &str) -> bool {
    matches!(
        line.trim_end_matches([' ', '\t']),
        "STYLE" | "REGION" | "NOTE"
    ) || line.starts_with("NOTE ")
        || line.starts_with("NOTE\t")
}
fn append_block(
    cues: &mut Vec<Cue>,
    block: &mut Vec<(usize, &str)>,
    format: Format,
) -> Result<(), String> {
    if block.is_empty() {
        return Ok(());
    }
    if format == Format::WebVtt && vtt_metadata(block[0].1) {
        block.clear();
        return Ok(());
    }
    if cues.len() >= MAX_CUES {
        return Err("Subtitle preview is limited to 100,000 cues".into());
    }
    let cue = parse_block(block, format)
        .map_err(|error| format!("Subtitle block {}: {error}", cues.len() + 1))?;
    cues.push(cue);
    block.clear();
    Ok(())
}
fn parse_block(block: &[(usize, &str)], format: Format) -> Result<Cue, String> {
    let timing = usize::from(!block[0].1.contains("-->"));
    let identifier = if timing == 1 {
        let number = if format == Format::SubRip {
            block[0].1.trim()
        } else {
            block[0].1
        };
        if format == Format::SubRip
            && (number.len() > 64 || !number.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err("Invalid SubRip counter".into());
        }
        if number.len() > 4096 {
            return Err("Cue identifier exceeds 4096 bytes".into());
        }
        number.to_owned()
    } else {
        String::new()
    };
    let (source_line, text) = block.get(timing).ok_or("Missing cue timing")?;
    let (start, right) = text
        .split_once("-->")
        .ok_or("Missing --> timestamp separator")?;
    let right = right.trim();
    let end = right
        .split_whitespace()
        .next()
        .ok_or("Missing end timestamp")?;
    let start_ms = timestamp(start.trim(), format)?;
    let end_ms = timestamp(end, format)?;
    if end_ms < start_ms {
        return Err("Cue ends before it starts".into());
    }
    if format == Format::WebVtt && end_ms == start_ms {
        return Err("WebVTT cue must have a positive duration".into());
    }
    Ok(Cue {
        identifier,
        start_ms,
        end_ms,
        source_line: *source_line,
        settings: right[end.len()..].trim().to_owned(),
        text: block[timing + 1..]
            .iter()
            .map(|(_, text)| *text)
            .collect::<Vec<_>>()
            .join("\n"),
    })
}
fn timestamp(text: &str, format: Format) -> Result<u64, String> {
    let separator = if format == Format::SubRip { ',' } else { '.' };
    let (clock, fraction) = text
        .split_once(separator)
        .ok_or("Missing timestamp milliseconds")?;
    let fraction = milliseconds(fraction, format)?;
    let fields = clock_fields(clock, format)?;
    let hours = decimal(fields[0])?;
    let minutes = decimal(fields[1])?;
    let seconds = decimal(fields[2])?;
    if fields[1].len() != 2 || fields[2].len() != 2 || minutes >= 60 || seconds >= 60 {
        return Err("Minutes and seconds must be 00–59".into());
    }
    hours
        .checked_mul(3_600_000)
        .and_then(|value| value.checked_add(minutes * 60_000 + seconds * 1000 + fraction))
        .ok_or_else(|| "Timestamp is too large".into())
}
fn milliseconds(text: &str, format: Format) -> Result<u64, String> {
    let (digits, scale) = match format {
        Format::AdvancedSubStationAlpha => (2, 10),
        _ => (3, 1),
    };
    if text.len() != digits {
        return Err(format!("Timestamp fraction requires {digits} digits"));
    }
    Ok(decimal(text)? * scale)
}
fn clock_fields(clock: &str, format: Format) -> Result<[&str; 3], String> {
    let mut fields = clock.split(':').collect::<Vec<_>>();
    if format == Format::WebVtt && fields.len() == 2 {
        fields.insert(0, "00");
    }
    let fields: [&str; 3] = fields
        .try_into()
        .map_err(|_| "Expected hours:minutes:seconds")?;
    if format == Format::WebVtt && fields[0].len() < 2 {
        return Err("WebVTT hours require at least two digits".into());
    }
    Ok(fields)
}
fn decimal(text: &str) -> Result<u64, String> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Timestamp fields require ASCII digits".into());
    }
    text.parse().map_err(|_| "Timestamp is too large".into())
}

/// Returns whether a path is a supported subtitle source.
pub fn is_subtitle_path(path: &Path) -> bool {
    Format::from_path(path).is_some()
}
/// Formats milliseconds as an hour-inclusive media timecode.
pub fn format_time(time_ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        time_ms / 3_600_000,
        time_ms / 60_000 % 60,
        time_ms / 1000 % 60,
        time_ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn srt_preserves_counters_markup_multiline_payload_and_source_lines() {
        let source = "\u{feff}7\r\n00:00:01,250 --> 00:00:03,000 X1:10\r\n<i>Hello</i>\r\nworld\r\n\r\n8\r\n00:00:02,500 --> 00:00:04,000\r\nOverlap\r\n";
        let doc = Document::parse_srt(source, &AtomicBool::new(false)).unwrap();
        assert_eq!(doc.cues.len(), 2);
        assert_eq!(doc.cues[0].identifier, "7");
        assert_eq!(doc.cues[0].text, "<i>Hello</i>\nworld");
        assert_eq!(doc.cues[0].source_line, 2);
        assert_eq!(doc.cues[1].source_line, 7);
        assert_eq!(doc.cues[0].settings, "X1:10");
        assert_eq!(doc.timeline.peak_concurrency, 2);
        assert_eq!(format_time(doc.cues[0].start_ms), "00:00:01.250");
    }
    #[test]
    fn srt_rejects_bad_intervals_timestamps_and_oversized_inputs() {
        for source in [
            "1\n00:00:60,000 --> 00:01:00,000\nbad",
            "1\n00:00:02,000 --> 00:00:01,000\nbad",
            "1\n00:00:01.000 --> 00:00:02.000\nbad",
            "counter\n00:00:01,000 --> 00:00:02,000\nbad",
            "1\n9999999999999999999:00:01,000 --> 9999999999999999999:00:02,000\nbad",
        ] {
            assert!(
                Document::parse_srt(source, &AtomicBool::new(false)).is_err(),
                "{source}"
            );
        }
        assert!(Document::parse_srt(&" ".repeat(MAX_BYTES + 1), &AtomicBool::new(false)).is_err());
        assert!(
            Document::parse_srt(
                "1\n00:00:01,000 --> 00:00:02,000\ntext",
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
