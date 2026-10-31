//! Sparse visual samples plus the selected audio track, with source timestamps.
use super::{Engine, Path, Prepared, STANDARD, media};
use crate::file_manager::media::selected_video_stream;
use serde_json::Value;

const DEMUXERS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,avi,mpeg,mpegts,ogg,flv,asf";
const MAX_FRAMES: usize = 1024;

pub(super) fn prepare(path: &Path, cursor: usize, bytes: u64) -> Result<Prepared, String> {
    if bytes > 64 * 1024 * 1024 * 1024 {
        return Ok(Prepared {
            coverage: "name-only: video exceeds 64 GiB source budget".into(),
            ..Default::default()
        });
    }
    let metadata = media::probe(path, DEMUXERS)?;
    let visual = visual_track(&metadata)?;
    let audio = media::audio_track(&metadata);
    let count = visual.map_or(0, |track| frame_count(track.duration));
    let mut prepared = if cursor < count {
        let track = visual.unwrap();
        frame(path, &metadata, &track, sample_time(track.duration, cursor))?
    } else if let Some(track) = audio {
        let mut prepared = media::audio_window(path, DEMUXERS, &track, cursor - count)?;
        prepared.next = prepared.next.map(|next| next + count);
        prepared
    } else {
        return Err("No supported video or audio sample at this position".into());
    };
    if cursor < count {
        prepared.next = (cursor + 1 < count || audio.is_some()).then_some(cursor + 1);
    }
    if visual.is_some() {
        prepared.coverage = if prepared.coverage.starts_with("partial:") {
            format!("sampled: visual frames; {}", prepared.coverage)
        } else if audio.is_some_and(|track| track.duration > 7200.0) {
            "sampled: visual frames; partial audio, first two hours".into()
        } else if audio.is_some() {
            "sampled: visual frames; selected audio track".into()
        } else {
            "sampled: visual frames; no audio track observed".into()
        };
    }
    Ok(prepared)
}

fn visual_track(metadata: &Value) -> Result<Option<media::Track>, String> {
    selected_video_stream(metadata)
        .ok()
        .map(|(_, stream)| {
            let width = stream["width"].as_u64().unwrap_or(u64::MAX);
            let height = stream["height"].as_u64().unwrap_or(u64::MAX);
            if width == 0
                || height == 0
                || width > 8192
                || height > 8192
                || width.saturating_mul(height) > 4096 * 2160
            {
                return Err("Video dimensions exceed extraction budget".to_owned());
            }
            media::Track::from_stream(metadata, stream)
                .ok_or_else(|| "Video duration unavailable".into())
        })
        .transpose()
}

fn frame_count(duration: f64) -> usize {
    (duration / 5.0).ceil().clamp(1.0, MAX_FRAMES as f64) as usize
}
fn sample_time(duration: f64, index: usize) -> f64 {
    if duration > MAX_FRAMES as f64 * 5.0 {
        duration * (index as f64 + 0.5) / MAX_FRAMES as f64
    } else {
        let start = index as f64 * 5.0;
        (start + (start + 5.0).min(duration)) / 2.0
    }
}
fn frame(
    path: &Path,
    metadata: &Value,
    track: &media::Track,
    offset: f64,
) -> Result<Prepared, String> {
    let decode = |accurate: bool| {
        let mut command = media::command("ffmpeg", DEMUXERS).args([
            "-v",
            "info",
            "-nostdin",
            "-threads",
            "2",
            "-filter_threads",
            "1",
            "-max_alloc",
            "268435456",
            "-copyts",
        ]);
        if !accurate {
            command = command.arg("-noaccurate_seek");
        }
        command
            .arg("-ss")
            .arg((track.start + offset).to_string())
            .arg("-i")
            .arg_content_file(path)
            .map_err(|e| e.to_string())?
            .args([
                "-map",
                &format!("0:{}", track.index),
                "-an",
                "-sn",
                "-dn",
                "-vf",
                "scale=1024:1024:force_original_aspect_ratio=decrease,showinfo",
                "-frames:v",
                "1",
                "-fps_mode",
                "passthrough",
                "-c:v",
                "png",
                "-f",
                "image2pipe",
                "pipe:1",
            ])
            .max_output_bytes(8 * 1024 * 1024)
            .run()
            .map_err(|e| e.to_string())
    };
    let mut output = decode(true)?;
    // A valid sparse/one-frame clip can have no frame after the requested
    // instant. Retry once at its preceding demuxer seek point; showinfo keeps
    // the observed frame's timestamp rather than inventing the requested one.
    if output.status.success()
        && !output.stdout_truncated
        && output.stdout.is_empty()
        && offset > 0.0
    {
        output = decode(false)?;
    }
    if !output.status.success() || output.stdout_truncated || output.stdout.is_empty() {
        return Err("Video frame decode failed".into());
    }
    let origin = media::number(&metadata["format"]["start_time"]).unwrap_or(0.0);
    let timestamp =
        frame_timestamp(&output.stderr).ok_or("Decoded video timestamp unavailable")? - origin;
    if timestamp < 0.0 || timestamp > track.start + track.duration + 1.0 {
        return Err("Decoded video timestamp outside source duration".into());
    }
    Ok(Prepared {
        data: Some(STANDARD.encode(&output.stdout)),
        modality: "image".into(),
        location: format!("video frame at seconds {timestamp:.6}"),
        vision_tokens: Some(140),
        ..Default::default()
    })
}
fn frame_timestamp(stderr: &[u8]) -> Option<f64> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| line.contains("Parsed_showinfo_"))
        .find_map(|line| {
            let (_, after) = line.split_once("pts_time:")?;
            after
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
                .filter(|time| time.is_finite())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sampling_covers_short_and_capped_long_clips_without_seeking_eof() {
        assert_eq!(frame_count(0.1), 1);
        assert_eq!(sample_time(0.1, 0), 0.05);
        assert_eq!(frame_count(11.0), 3);
        assert_eq!(sample_time(11.0, 2), 10.5);
        assert_eq!(frame_count(10000.0), MAX_FRAMES);
        assert!(sample_time(10000.0, 0) > 0.0);
        assert!(sample_time(10000.0, MAX_FRAMES - 1) < 10000.0);
        assert!(sample_time(10000.0, MAX_FRAMES - 1) > 9990.0);
    }
    #[test]
    fn source_timestamp_requires_the_decoded_frame_record() {
        assert_eq!(
            frame_timestamp(
                b"Duration: 10\n[Parsed_showinfo_1 @ 0x12] n: 0 pts: 81920 pts_time:5.000 pos: 5\n"
            ),
            Some(5.0)
        );
        assert_eq!(frame_timestamp(b"other pts_time:5.0\n"), None);
        assert_eq!(frame_timestamp(b"[Parsed_showinfo_0] pts_time:nan\n"), None);
    }
    #[test]
    fn capped_long_sparse_video_keeps_the_last_observed_frame() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("long.mkv");
        let output = little_clip_core::process::CommandRequest::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=80x40:r=1/10:d=10000",
                "-vf",
                r"drawbox=color=blue:t=fill:enable=gte(t\,9980)",
                "-c:v",
                "ffv1",
                "-g",
                "1",
            ])
            .arg(&path)
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        let bytes = std::fs::metadata(&path).unwrap().len();
        for (cursor, blue) in [(0, false), (512, false), (1023, true)] {
            let prepared = prepare(&path, cursor, bytes).unwrap();
            assert_eq!(prepared.vision_tokens, Some(140));
            assert_eq!(prepared.next, (cursor < 1023).then_some(cursor + 1));
            assert_eq!(
                prepared.coverage,
                "sampled: visual frames; no audio track observed"
            );
            let timestamp: f64 = prepared
                .location
                .rsplit(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            if blue {
                assert_eq!(timestamp, 9990.0);
            }
            let image = image::load_from_memory(&STANDARD.decode(prepared.data.unwrap()).unwrap())
                .unwrap()
                .to_rgb8();
            assert!(image.get_pixel(0, 0)[if blue { 2 } else { 0 }] > 200);
        }
    }
    fn clip(path: &Path, offset: &str, length: &str) {
        assert!(
            std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=c=blue:s=80x40:r=10:d={length}"),
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency=440:duration={length}"),
                    "-c:v",
                    "ffv1",
                    "-c:a",
                    "pcm_s16le",
                    "-output_ts_offset",
                    offset,
                ])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }
    #[test]
    fn video_frames_and_audio_keep_real_timestamps_including_nonzero_container_origin() {
        let folder = tempfile::tempdir().unwrap();
        for (name, origin, length, frame_times) in [
            ("normal.mkv", "0", "6", vec![2.5, 5.5]),
            ("offset.mkv", "5", "1", vec![0.5]),
        ] {
            let path = folder.path().join(name);
            clip(&path, origin, length);
            let size = std::fs::metadata(&path).unwrap().len();
            for (cursor, expected) in frame_times.iter().enumerate() {
                let prepared = prepare(&path, cursor, size).unwrap();
                assert_eq!(prepared.modality, "image");
                assert_eq!(prepared.vision_tokens, Some(140));
                assert_eq!(prepared.next, Some(cursor + 1));
                let actual: f64 = prepared
                    .location
                    .rsplit(' ')
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!((actual - expected).abs() < 0.01, "{}", prepared.location);
                let png = STANDARD.decode(prepared.data.unwrap()).unwrap();
                let image = image::load_from_memory(&png).unwrap();
                assert_eq!((image.width(), image.height()), (1024, 512));
                assert!(image.to_rgb8().get_pixel(0, 0)[2] > 200);
            }
            let audio = prepare(&path, frame_times.len(), size).unwrap();
            assert_eq!(audio.modality, "audio");
            assert_eq!(audio.next, None);
            assert!(audio.coverage.starts_with("sampled:"));
            let wav = STANDARD.decode(audio.data.unwrap()).unwrap();
            let seconds = (wav.len() - 44) as f64 / 32000.0;
            assert!(
                (seconds - length.parse::<f64>().unwrap()).abs() < 0.02,
                "{seconds}"
            );
        }
    }
    #[test]
    fn a_one_frame_video_uses_the_available_frame_with_its_actual_timestamp() {
        let folder = tempfile::tempdir().unwrap();
        for origin in ["0", "5"] {
            let path = folder.path().join(format!("single-{origin}.mkv"));
            assert!(
                std::process::Command::new("ffmpeg")
                    .args([
                        "-v",
                        "error",
                        "-f",
                        "lavfi",
                        "-i",
                        "color=c=red:s=80x40:r=1:d=1",
                        "-c:v",
                        "ffv1",
                        "-output_ts_offset",
                        origin,
                    ])
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            let prepared = prepare(&path, 0, std::fs::metadata(&path).unwrap().len()).unwrap();
            assert_eq!(prepared.location, "video frame at seconds 0.000000");
            assert_eq!(prepared.next, None);
            assert!(prepared.coverage.starts_with("sampled:"));
            let image =
                image::load_from_memory(&STANDARD.decode(prepared.data.unwrap()).unwrap()).unwrap();
            assert!(image.to_rgb8().get_pixel(0, 0)[0] > 200);
        }
    }

    #[test]
    fn packet_timestamp_gaps_keep_later_audio_windows_on_the_source_clock() {
        let folder = tempfile::tempdir().unwrap();
        let original = folder.path().join("original.mkv");
        clip(&original, "0", "60");
        for visual in [false, true] {
            let path = folder.path().join(if visual {
                "gapped-video.mkv"
            } else {
                "gapped-audio.mkv"
            });
            let mut command = std::process::Command::new("ffmpeg");
            command.args(["-v", "error", "-i"]).arg(&original).args([
                "-af",
                r"aselect=not(between(t\,10\,12))",
                "-c:v",
                "copy",
                "-c:a",
                "pcm_s16le",
            ]);
            if !visual {
                command.arg("-vn");
            }
            assert!(command.arg(&path).status().unwrap().success());
            let frames = if visual { 12 } else { 0 };
            for (offset, length, next) in [(0, 30, Some(25)), (25, 30, Some(50)), (50, 10, None)] {
                let prepared = prepare(
                    &path,
                    frames + offset,
                    std::fs::metadata(&path).unwrap().len(),
                )
                .unwrap();
                let wav = STANDARD.decode(prepared.data.unwrap()).unwrap();
                assert_eq!((wav.len() - 44) / 32000, length);
                assert_eq!(
                    prepared.location,
                    format!("audio seconds {offset}..{}", offset + length)
                );
                assert_eq!(prepared.next, next.map(|offset| frames + offset));
                if visual {
                    assert_eq!(
                        prepared.coverage,
                        "sampled: visual frames; selected audio track"
                    );
                } else {
                    assert_eq!(prepared.coverage, "complete");
                }
                assert!(wav[44..].iter().any(|byte| *byte != 0));
                if offset == 0 {
                    assert!(
                        wav[44 + 10 * 32000 + 16000..44 + 11 * 32000 + 16000]
                            .iter()
                            .all(|byte| *byte == 0)
                    );
                }
            }
        }
    }

    #[test]
    fn video_playlist_disguised_as_mp4_cannot_fetch_network_content() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("video.mp4");
        std::fs::write(&path, format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nhttp://{}/segment.ts\n#EXT-X-ENDLIST\n", listener.local_addr().unwrap())).unwrap();
        assert!(prepare(&path, 0, std::fs::metadata(&path).unwrap().len()).is_err());
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}
