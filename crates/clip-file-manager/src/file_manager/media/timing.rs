//! Shared source-relative stream timing from bounded FFprobe metadata.
use serde_json::Value;

pub(in crate::file_manager) fn number(value: &Value) -> Option<f64> {
    value
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && value.abs() < 9.0e9)
}
#[derive(Clone, Copy)]
pub(in crate::file_manager) struct Track {
    pub index: usize,
    pub start: f64,
    pub duration: f64,
}
impl Track {
    pub fn from_stream(value: &Value, stream: &Value) -> Option<Self> {
        let origin = number(&value["format"]["start_time"]).unwrap_or(0.0);
        let relative = number(&stream["start_time"]).unwrap_or(origin) - origin;
        let start = relative.max(0.0);
        let duration = number(&stream["duration"])
            .map(|length| length + relative - start)
            .or_else(|| {
                let end = duration_tag(stream["tags"]["DURATION"].as_str()?)?;
                Some(end - origin - start)
            })
            .or_else(|| number(&value["format"]["duration"]).map(|length| length - start))?;
        (duration > 0.0).then_some(Self {
            index: stream["index"].as_u64()? as usize,
            start,
            duration,
        })
    }
}
// Matroska DURATION tags express the stream's end on the container timeline.
fn duration_tag(value: &str) -> Option<f64> {
    let parts = value
        .split(':')
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if parts.len() != 3
        || parts.iter().any(|part| !part.is_finite() || *part < 0.0)
        || parts[1] >= 60.0
        || parts[2] >= 60.0
    {
        return None;
    }
    let seconds = parts[0] * 3600.0 + parts[1] * 60.0 + parts[2];
    (seconds < 9.0e9).then_some(seconds)
}
