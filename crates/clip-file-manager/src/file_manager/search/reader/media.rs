//! Shared bounded media commands and canonical audio windows.
use super::{CommandRequest, Duration, Engine, Path, Prepared, STANDARD};
use serde_json::Value;

pub(super) fn command(program: &str, demuxers: &str) -> CommandRequest {
    CommandRequest::new(program)
        .die_with_parent()
        .args([
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            demuxers,
        ])
        .timeout(Duration::from_secs(30))
}
pub(super) fn probe(path: &Path, demuxers: &str) -> Result<Value, String> {
    let output = command("ffprobe", demuxers)
        .args(["-v", "error", "-show_entries", "format=duration,start_time:stream=index,codec_type,duration,start_time,width,height,avg_frame_rate,r_frame_rate:stream_disposition=attached_pic,default:stream_tags=DURATION", "-of", "json"])
        .arg_content_file(path).map_err(|e| e.to_string())?
        .max_output_bytes(64 * 1024).run().map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err("Media metadata unavailable or unsupported container".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}
pub(super) use crate::file_manager::media::timing::{Track, number};

pub(super) fn audio_track(value: &Value) -> Option<Track> {
    value["streams"]
        .as_array()?
        .iter()
        .filter(|stream| stream["codec_type"] == "audio")
        .min_by_key(|stream| stream["disposition"]["default"].as_u64() != Some(1))
        .and_then(|stream| Track::from_stream(value, stream))
}
pub(super) fn audio_window(
    path: &Path,
    demuxers: &str,
    track: &Track,
    cursor: usize,
) -> Result<Prepared, String> {
    let offset = cursor as f64;
    let limit = track.duration.min(7200.0);
    if offset >= limit {
        return Err("Audio position is outside extraction budget".into());
    }
    let start = track.start + offset;
    let length = (limit - offset).min(30.0);
    let output = command("ffmpeg", demuxers)
        .args([
            "-v",
            "error",
            "-nostdin",
            "-threads",
            "2",
            "-filter_threads",
            "1",
            "-ss",
        ])
        .arg(start.to_string())
        .arg("-i")
        .arg_content_file(path)
        .map_err(|e| e.to_string())?
        .args([
            "-map",
            &format!("0:{}", track.index),
            "-t",
            &length.to_string(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-af",
            "aresample=16000:async=1:min_hard_comp=0:first_pts=0",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ])
        .timeout(Duration::from_secs(60))
        .max_output_bytes(2 * 1024 * 1024)
        .run()
        .map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err("Audio decode failed".into());
    }
    let pcm = output.stdout;
    let decoded = pcm.len() as f64 / 32000.0;
    let wav = wav(&pcm)?;
    Ok(Prepared {
        data: Some(STANDARD.encode(wav)),
        modality: "audio".into(),
        location: format!("audio seconds {start}..{}", start + decoded),
        next: (offset + length < limit && decoded + 0.001 >= length).then_some(cursor + 25),
        coverage: if decoded + 0.05 < length {
            "partial: audio ended before reported duration"
        } else if track.duration > 7200.0 {
            "partial: first two hours of audio track"
        } else {
            "complete"
        }
        .into(),
        ..Default::default()
    })
}
fn wav(pcm: &[u8]) -> Result<Vec<u8>, String> {
    if pcm.is_empty() || !pcm.len().is_multiple_of(2) {
        return Err("Invalid decoded PCM".into());
    }
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16000_u32.to_le_bytes());
    wav.extend_from_slice(&32000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    Ok(wav)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::io::Write;

    #[test]
    fn quiet_windows_cover_short_tails_and_stop_at_the_two_hour_horizon() {
        let folder = tempfile::tempdir().unwrap();
        for (seconds, expected_count) in [
            (1.25_f64, 1),
            (30.0, 1),
            (30.5, 2),
            (51.0, 2),
            (55.5, 3),
            (7201.0, 1),
        ] {
            let path = folder.path().join(format!("{seconds}.wav"));
            let data_size = (seconds * 32000.0) as u32;
            let mut header = wav(&[0, 0]).unwrap();
            header[4..8].copy_from_slice(&(36 + data_size).to_le_bytes());
            header[40..44].copy_from_slice(&data_size.to_le_bytes());
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(&header[..44]).unwrap();
            file.set_len(44 + u64::from(data_size)).unwrap();
            drop(file);
            let track = audio_track(&probe(&path, "wav").unwrap()).unwrap();
            let mut cursor = if seconds > 7200.0 { 7175 } else { 0 };
            let mut count = 0;
            loop {
                let prepared = audio_window(&path, "wav", &track, cursor).unwrap();
                let bytes = STANDARD.decode(prepared.data.unwrap()).unwrap();
                let end = (cursor as f64 + 30.0).min(seconds.min(7200.0));
                assert_eq!((bytes.len() - 44) as f64 / 32000.0, end - cursor as f64);
                assert_eq!(prepared.location, format!("audio seconds {cursor}..{end}"));
                assert_eq!(&bytes[22..24], &1_u16.to_le_bytes());
                assert_eq!(&bytes[24..28], &16000_u32.to_le_bytes());
                assert_eq!(&bytes[34..36], &16_u16.to_le_bytes());
                assert!(
                    bytes[44..].iter().all(|byte| *byte == 0),
                    "Quiet input must remain represented"
                );
                assert_eq!(prepared.coverage.starts_with("partial:"), seconds > 7200.0);
                count += 1;
                let Some(next) = prepared.next else { break };
                assert_eq!(next, cursor + 25);
                cursor = next;
            }
            assert_eq!(count, expected_count);
            assert!(audio_window(&path, "wav", &track, 7200).is_err());
        }
    }
}
