//! Passive subtitle cue parsing and bounded timeline summaries.
mod timeline;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
pub use timeline::{Bin, Timeline};

/// Maximum encoded subtitle text accepted by the cue reader.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_CUES: usize = 100_000;

/// One subtitle cue, in source-file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    /// Original counter or identifier, when present.
    pub identifier: String,
    /// Inclusive start time, in milliseconds from the media origin.
    pub start_ms: u64,
    /// Exclusive end time; zero-duration cues remain inspectable markers.
    pub end_ms: u64,
    /// Caption lines joined with line feeds, with formatting text preserved.
    pub text: String,
    /// Positioning metadata following the end timestamp.
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
        if cancelled.load(Ordering::Relaxed) {
            return Err("Subtitle loading cancelled".into());
        }
        if source.len() > MAX_BYTES {
            return Err("Subtitle preview is limited to 8 MiB".into());
        }
        let mut cues = Vec::new();
        let mut block = Vec::new();
        for (line, text) in source.trim_start_matches('\u{feff}').lines().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Subtitle loading cancelled".into());
            }
            if text.trim().is_empty() {
                append_block(&mut cues, &mut block)?;
            } else {
                block.push((line + 1, text));
            }
        }
        append_block(&mut cues, &mut block)?;
        let timeline = Timeline::build(&cues, cancelled)?;
        Ok(Self { cues, timeline })
    }
}
fn append_block(cues: &mut Vec<Cue>, block: &mut Vec<(usize, &str)>) -> Result<(), String> {
    if block.is_empty() {
        return Ok(());
    }
    if cues.len() >= MAX_CUES {
        return Err("Subtitle preview is limited to 100,000 cues".into());
    }
    let cue = parse_block(block)
        .map_err(|error| format!("Subtitle block {}: {error}", cues.len() + 1))?;
    cues.push(cue);
    block.clear();
    Ok(())
}
fn parse_block(block: &[(usize, &str)]) -> Result<Cue, String> {
    let timing = usize::from(!block[0].1.contains("-->"));
    let identifier = if timing == 1 {
        let number = block[0].1.trim();
        if number.len() > 64 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("Invalid SubRip counter".into());
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
    let start_ms = timestamp(start.trim(), ',')?;
    let end_ms = timestamp(end, ',')?;
    if end_ms < start_ms {
        return Err("Cue ends before it starts".into());
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
fn timestamp(text: &str, separator: char) -> Result<u64, String> {
    let (clock, fraction) = text
        .split_once(separator)
        .ok_or("Missing timestamp milliseconds")?;
    if fraction.len() != 3 {
        return Err("Milliseconds require three digits".into());
    }
    let fraction = decimal(fraction)?;
    let fields: [&str; 3] = clock
        .split(':')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "Expected hours:minutes:seconds")?;
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
fn decimal(text: &str) -> Result<u64, String> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Timestamp fields require ASCII digits".into());
    }
    text.parse().map_err(|_| "Timestamp is too large".into())
}

/// Returns whether a path is a supported subtitle source.
pub fn is_subtitle_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("srt"))
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
