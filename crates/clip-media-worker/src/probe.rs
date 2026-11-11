//! Bounded metadata extraction over owned custom stream I/O.

use ffmpeg_next as ffmpeg;
use std::{
    fs::File,
    sync::OnceLock,
    time::{Duration, Instant},
};

pub(super) fn probe(file: File, formats: &str) -> Result<serde_json::Value, String> {
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Media input must be ordinary file content".into());
    }
    static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();
    INITIALIZED
        .get_or_init(|| {
            ffmpeg::init().map_err(|error| error.to_string())?;
            ffmpeg::log::set_level(ffmpeg::log::Level::Error);
            // SAFETY: set the process-global allocation cap once, before decoding.
            // This worker is independent of the desktop process and other jobs.
            unsafe { ffmpeg::ffi::av_max_alloc(64 * 1024 * 1024) };
            Ok(())
        })
        .clone()?;
    let io = ffmpeg::format::context::StreamIo::from_read_seek(file)
        .map_err(|error| error.to_string())?;
    let mut options = ffmpeg::Dictionary::new();
    // All primary reads go through custom I/O. No decoder may open secondary
    // files or network resources through FFmpeg's URL protocols.
    options.set("protocol_whitelist", "");
    options.set("format_whitelist", formats);
    options.set("probesize", "8388608");
    options.set("analyzeduration", "3000000");
    options.set("max_streams", "128");
    let deadline = Instant::now() + Duration::from_secs(30);
    let input =
        ffmpeg::format::input_from_stream_with_interrupt(io, None, Some(options), move || {
            Instant::now() >= deadline
        })
        .map_err(|error| error.to_string())?;
    let streams: Vec<_> = input.streams().map(stream_metadata).collect();
    // SAFETY: input owns this live format context for the duration of the read.
    let start_time = unsafe { (*input.as_ptr()).start_time };
    Ok(serde_json::json!({
        "streams": streams,
        "format": {
            "start_time": (start_time != ffmpeg::ffi::AV_NOPTS_VALUE).then(|| (start_time as f64 / 1_000_000.0).to_string()),
            "duration": (input.duration() != ffmpeg::ffi::AV_NOPTS_VALUE).then(|| (input.duration() as f64 / 1_000_000.0).to_string()),
            "bit_rate": input.bit_rate().to_string(),
            "size": metadata.len().to_string(),
            "tags": tags(input.metadata()),
        },
    }))
}

fn stream_metadata(stream: ffmpeg::Stream<'_>) -> serde_json::Value {
    let parameters = stream.parameters();
    let medium = match parameters.medium() {
        ffmpeg::media::Type::Audio => "audio",
        ffmpeg::media::Type::Video => "video",
        ffmpeg::media::Type::Subtitle => "subtitle",
        _ => "data",
    };
    let time_base = stream.time_base();
    let seconds = |timestamp: i64| {
        (timestamp != ffmpeg::ffi::AV_NOPTS_VALUE && time_base.denominator() != 0)
            .then(|| (timestamp as f64 * f64::from(time_base)).to_string())
    };
    // SAFETY: Parameters owns a live AVCodecParameters allocation, and
    // this immutable borrow does not outlive it or mutate the context.
    let raw = unsafe { &*parameters.as_ptr() };
    let mut layout = [0_i8; 128];
    // SAFETY: the layout belongs to the live parameters; the output is
    // an initialized writable buffer with the specified capacity.
    let layout_result = unsafe {
        ffmpeg::ffi::av_channel_layout_describe(&raw.ch_layout, layout.as_mut_ptr(), layout.len())
    };
    let layout = if raw.ch_layout.order != ffmpeg::ffi::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC
        && layout_result >= 0
        && (layout_result as usize) < layout.len()
    {
        // SAFETY: a successful non-truncated describe writes a NUL terminator.
        Some(
            unsafe { std::ffi::CStr::from_ptr(layout.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        None
    };
    // SAFETY: this function returns either null or a static C string.
    let profile = unsafe { ffmpeg::ffi::avcodec_profile_name(raw.codec_id, raw.profile) };
    let profile = if profile.is_null() {
        None
    } else {
        Some(
            unsafe { std::ffi::CStr::from_ptr(profile) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    let sample_formats = [
        "u8", "s16", "s32", "flt", "dbl", "u8p", "s16p", "s32p", "fltp", "dblp", "s64", "s64p",
    ];
    let mut value = serde_json::json!({
    "index": stream.index(),
    "codec_type": medium,
    "codec_name": parameters.id().name(),
        "profile": profile,
        "bit_rate": (raw.bit_rate > 0).then(|| raw.bit_rate.to_string()),
    "duration": seconds(stream.duration()),
    "start_time": seconds(stream.start_time()),
    "avg_frame_rate": format!("{}/{}", stream.avg_frame_rate().numerator(), stream.avg_frame_rate().denominator()),
    "r_frame_rate": format!("{}/{}", stream.rate().numerator(), stream.rate().denominator()),
    "tags": tags(stream.metadata()),
    "disposition": {
        "attached_pic": u8::from(stream.disposition().contains(ffmpeg::format::stream::Disposition::ATTACHED_PIC)),
        "default": u8::from(stream.disposition().contains(ffmpeg::format::stream::Disposition::DEFAULT)),
    },
    });
    if medium == "audio" {
        value["sample_rate"] = raw.sample_rate.to_string().into();
        value["channels"] = raw.ch_layout.nb_channels.into();
        value["channel_layout"] = serde_json::json!(layout);
        value["bits_per_raw_sample"] =
            serde_json::json!((raw.bits_per_raw_sample > 0).then_some(raw.bits_per_raw_sample));
        // Coded-sample bookkeeping on lossy codecs is not a PCM bit depth.
        // SAFETY: codec_id is the enum supplied by the live codec parameters.
        value["bits_per_sample"] =
            unsafe { ffmpeg::ffi::av_get_bits_per_sample(raw.codec_id) }.into();
        value["sample_fmt"] = serde_json::json!(
            usize::try_from(raw.format)
                .ok()
                .and_then(|format| sample_formats.get(format))
        );
    } else if medium == "video" {
        value["width"] = raw.width.into();
        value["height"] = raw.height.into();
    }
    value
}

fn tags(dictionary: ffmpeg::DictionaryRef<'_>) -> serde_json::Value {
    dictionary
        .iter()
        .take(256)
        .map(|(key, value)| {
            (
                key.chars().take(128).collect(),
                serde_json::Value::String(value.chars().take(512).collect()),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, Write};

    #[test]
    fn ffmpeg_nine_probes_owned_seekable_pcm_without_url_protocols() {
        let mut file = tempfile::tempfile().unwrap();
        let samples = [0_u8; 1600];
        let mut wav = b"RIFF".to_vec();
        wav.extend((36_u32 + samples.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(8000_u32.to_le_bytes());
        wav.extend(16000_u32.to_le_bytes());
        wav.extend(2_u16.to_le_bytes());
        wav.extend(16_u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((samples.len() as u32).to_le_bytes());
        wav.extend(samples);
        file.write_all(&wav).unwrap();
        file.rewind().unwrap();
        let value = probe(file, "wav").unwrap();
        assert_eq!(value["streams"][0]["codec_name"], "pcm_s16le");
        assert_eq!(value["streams"][0]["duration"], "0.1");
        assert_eq!(value["streams"][0]["sample_rate"], "8000");
        assert_eq!(value["streams"][0]["channels"], 1);
    }

    #[test]
    fn invalid_media_is_rejected() {
        assert!(probe(tempfile::tempfile().unwrap(), "wav").is_err());
    }

    #[test]
    fn lossy_audio_does_not_report_coded_sample_bits_as_pcm_depth() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(include_bytes!("../tests/fixtures/silence.aac"))
            .unwrap();
        file.rewind().unwrap();
        let value = probe(file, "aac").unwrap();
        assert_eq!(value["streams"][0]["codec_name"], "aac");
        assert_eq!(value["streams"][0]["bits_per_sample"], 0);
        assert!(value["streams"][0]["bits_per_raw_sample"].is_null());
    }

    #[test]
    fn device_descriptors_are_rejected_before_probing() {
        assert!(probe(File::open("/dev/null").unwrap(), "wav").is_err());
    }
}
