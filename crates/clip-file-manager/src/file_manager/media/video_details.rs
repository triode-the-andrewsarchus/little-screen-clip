//! Best-effort display metadata from the existing bounded FFprobe request.
use serde_json::Value;

pub(super) fn describe(
    value: &Value,
    video: &Value,
    duration_ms: u64,
    dimensions: (u32, u32),
) -> (String, String) {
    let mut fields = vec![
        duration(duration_ms),
        format!("{}×{}", dimensions.0, dimensions.1),
    ];
    if let Some(codec) = video["codec_name"].as_str() {
        fields.push(codec_label(codec, video["profile"].as_str()));
    }
    if let Some(fps) = frame_rate(video) {
        fields.push(format!(
            "{} fps",
            format!("{fps:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
        ));
    }
    if let Some(rate) = video_bitrate(video) {
        fields.push(if rate >= 1_000_000 {
            format!("{:.2} Mbps", rate as f64 / 1_000_000.0)
        } else {
            format!("{:.0} kbps", rate as f64 / 1000.0)
        });
    }
    if let Some(year) = release_year(value, video) {
        fields.push(year.to_string());
    }
    (fields.join(" · "), audio_description(value))
}

fn duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if days > 0 {
        format!(
            "{days:02}:{:02}:{:02}:{:02}",
            hours % 24,
            minutes % 60,
            seconds % 60
        )
    } else if hours > 0 {
        format!("{hours:02}:{:02}:{:02}", minutes % 60, seconds % 60)
    } else {
        format!("{minutes:02}:{:02}", seconds % 60)
    }
}

pub(super) fn frame_rate(stream: &Value) -> Option<f64> {
    ["avg_frame_rate", "r_frame_rate"]
        .into_iter()
        .find_map(|key| {
            let text = stream[key].as_str()?;
            let (numerator, denominator) = text.split_once('/')?;
            let rate = numerator.parse::<f64>().ok()? / denominator.parse::<f64>().ok()?;
            (rate.is_finite() && rate > 0.0 && rate <= 1000.0).then_some(rate)
        })
}

fn codec_label(codec: &str, profile: Option<&str>) -> String {
    let label = match codec {
        "h264" => "H.264",
        "hevc" => "H.265 / HEVC",
        "mpeg4" => "MPEG-4",
        "mpeg2video" => "MPEG-2",
        "ac3" => "Dolby Digital",
        "eac3" => "Dolby Digital Plus",
        "truehd" => "Dolby TrueHD",
        "dts" => profile
            .filter(|profile| profile.starts_with("DTS"))
            .unwrap_or("DTS"),
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        _ => return codec.to_ascii_uppercase().replace('_', " "),
    };
    label.into()
}

fn audio_description(value: &Value) -> String {
    let mut streams: Vec<_> = value["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stream| stream["codec_type"].as_str() == Some("audio"))
        .collect();
    streams.sort_by_key(|stream| stream["disposition"]["default"].as_u64() != Some(1));
    let mut formats = Vec::new();
    for stream in streams {
        let mut fields = Vec::new();
        if let Some(codec) = stream["codec_name"].as_str() {
            fields.push(codec_label(codec, stream["profile"].as_str()));
        }
        if let Some(layout) = channel_layout(stream) {
            fields.push(layout);
        }
        let label = fields.join(" ");
        if !label.is_empty() && !formats.contains(&label) {
            formats.push(label);
        }
    }
    if formats.is_empty() {
        if value["streams"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|stream| stream["codec_type"].as_str() == Some("audio"))
        {
            "Audio metadata unavailable".into()
        } else {
            "No audio".into()
        }
    } else {
        format!("Audio: {}", formats.join(" / "))
    }
}

fn channel_layout(stream: &Value) -> Option<String> {
    let layout = stream["channel_layout"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != "unknown");
    match layout {
        Some("stereo") => Some("Stereo".into()),
        Some("mono") => Some("Mono".into()),
        Some(layout) => Some(layout.into()),
        None => match stream["channels"].as_u64()? {
            0 => None,
            1 => Some("Mono".into()),
            2 => Some("Stereo".into()),
            count => Some(format!("{count} channels")),
        },
    }
}

fn video_bitrate(stream: &Value) -> Option<u64> {
    positive_integer(&stream["bit_rate"]).or_else(|| {
        stream["tags"]
            .as_object()?
            .iter()
            .filter(|(key, _)| {
                key.eq_ignore_ascii_case("BPS") || key.eq_ignore_ascii_case("BPS-eng")
            })
            .find_map(|(_, value)| positive_integer(value))
    })
}

fn positive_integer(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| *n > 0)
}

fn release_year(value: &Value, video: &Value) -> Option<u16> {
    for tags in [&value["format"]["tags"], &video["tags"]] {
        for wanted in [
            "year",
            "release_year",
            "release_date",
            "date_released",
            "originaldate",
            "date",
        ] {
            if let Some(year) = tags
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| key.eq_ignore_ascii_case(wanted))
                .find_map(|(_, tag)| year_number(tag.as_str()?.trim().get(..4)?))
            {
                return Some(year);
            }
        }
    }
    None
}

fn year_number(text: &str) -> Option<u16> {
    if text.len() != 4 {
        return None;
    }
    text.parse::<u16>()
        .ok()
        .filter(|year| (1888..=2100).contains(year))
}
