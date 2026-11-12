//! Exercise the real executable and descriptor protocol, including exec-time inheritance.
use std::{
    fs::File,
    io::Write,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::Command,
};

fn command(inputs: &[&File], mode: &str, formats: &str) -> Command {
    let descriptors: Vec<_> = inputs.iter().map(|input| input.as_raw_fd()).collect();
    let mut command = Command::new(env!("CARGO_BIN_EXE_clip-media-worker"));
    command.args([mode, formats]);
    command.args(descriptors.iter().map(|fd| format!("/proc/self/fd/{fd}")));
    // SAFETY: only the child descriptor table changes. fcntl is async-signal-safe,
    // and the parent test retains the live File through command completion.
    unsafe {
        command.pre_exec(move || {
            for descriptor in &descriptors {
                if libc::fcntl(*descriptor, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    command
}

#[test]
fn executable_probes_and_decodes_a_retained_descriptor_from_eof() {
    let mut input = tempfile::tempfile().unwrap();
    input
        .write_all(include_bytes!("fixtures/silence.aac"))
        .unwrap();
    let probe = command(&[&input], "probe", "aac").output().unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(value["streams"][0]["codec_name"], "aac");
    let decoded = command(&[&input], "decode-audio", "aac").output().unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert!(!decoded.stdout.is_empty());
    assert_eq!(decoded.stdout.len() % 8, 0);
    assert!(
        decoded
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .all(|sample| f32::from_le_bytes(*sample).is_finite())
    );
}

#[test]
fn executable_denies_secondary_files_even_when_they_exist() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("audio.aac"),
        include_bytes!("fixtures/silence.aac"),
    )
    .unwrap();
    let mut playlist = tempfile::tempfile().unwrap();
    playlist
        .write_all(b"ffconcat version 1.0\nfile audio.aac\n")
        .unwrap();
    let result = command(&[&playlist], "probe", "concat")
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("whitelist"));
}

fn render_settings(count: usize) -> clip_media_protocol::AudioRender {
    let mut job = clip_media_protocol::AudioRender::new(
        (0..count)
            .map(|_| clip_media_protocol::AudioInput {
                formats: "wav".into(),
                stream: None,
            })
            .collect(),
        "wav",
    );
    job.codec = Some("pcm_s16le".into());
    job
}

fn constant_wav(frames: usize, values: [i16; 2]) -> File {
    let file = tempfile::tempfile().unwrap();
    let mut writer = hound::WavWriter::new(
        file.try_clone().unwrap(),
        hound::WavSpec {
            channels: 2,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..frames {
        for value in values {
            writer.write_sample(value).unwrap();
        }
    }
    writer.finalize().unwrap();
    file
}

fn render(inputs: &[&File], job: &clip_media_protocol::AudioRender) -> File {
    let output = tempfile::tempfile().unwrap();
    let result = command(inputs, "render-audio", &serde_json::to_string(job).unwrap())
        .stdout(output.try_clone().unwrap())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    output
}

fn wav_samples(mut file: File) -> (hound::WavSpec, Vec<i16>) {
    use std::io::Seek;
    file.rewind().unwrap();
    let mut reader = hound::WavReader::new(file).unwrap();
    (
        reader.spec(),
        reader.samples::<i16>().map(Result::unwrap).collect(),
    )
}

#[test]
fn executable_filters_selected_frames_and_preserves_mono_channel_selection() {
    let input = constant_wav(16000, [1234, -2468]);
    let mut job = render_settings(1);
    job.range = Some((1.25, 1.5));
    job.filter = Some(clip_media_protocol::AudioFilter::Chain(
        "pan=mono|c0=c1,volume=0.5".into(),
    ));
    let (spec, samples) = wav_samples(render(&[&input], &job));
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.sample_rate, 8000);
    assert_eq!(samples.len(), 2000);
    assert!(samples.iter().all(|sample| *sample == -1234));
}

#[test]
fn executable_concatenates_inputs_and_flushes_the_graph_and_encoder() {
    let first = constant_wav(800, [1000, -1000]);
    let second = constant_wav(400, [2000, -2000]);
    let mut job = render_settings(2);
    job.filter = Some(clip_media_protocol::AudioFilter::Graph {
        spec: "[0:a][1:a]concat=n=2:v=0:a=1[out]".into(),
        output: "out".into(),
    });
    let (spec, samples) = wav_samples(render(&[&first, &second], &job));
    assert_eq!(spec.channels, 2);
    assert_eq!(samples.len(), 2400);
    assert!(
        samples[..1600]
            .as_chunks::<2>()
            .0
            .iter()
            .all(|frame| *frame == [1000, -1000])
    );
    assert!(
        samples[1600..]
            .as_chunks::<2>()
            .0
            .iter()
            .all(|frame| *frame == [2000, -2000])
    );
}

#[test]
fn executable_writes_seekable_lossless_and_lossy_containers() {
    let input = constant_wav(4000, [1234, -2468]);
    for (format, codec) in [
        ("flac", "flac"),
        ("mp3", "libmp3lame"),
        ("ipod", "aac"),
        ("ogg", "libvorbis"),
        ("opus", "libopus"),
        ("asf", "wmav2"),
        ("aiff", "pcm_s16be"),
    ] {
        let mut job = render_settings(1);
        job.format = format.into();
        job.codec = Some(codec.into());
        let output = render(&[&input], &job);
        let result = command(&[&output], "probe", "flac,mp3,mov,ogg,asf,aiff")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["streams"][0]["codec_type"], "audio");
        let decoded = command(&[&output], "decode-audio", "flac,mp3,mov,ogg,asf,aiff")
            .output()
            .unwrap();
        assert!(
            decoded.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&decoded.stderr)
        );
        assert!(!decoded.stdout.is_empty());
    }
}

#[test]
fn executable_rejects_non_seekable_outputs_and_file_opening_audio_filters() {
    let input = constant_wav(800, [1234, -2468]);
    let mut job = render_settings(1);
    let result = command(
        &[&input],
        "render-audio",
        &serde_json::to_string(&job).unwrap(),
    )
    .output()
    .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("seekable staging"));
    let output = tempfile::tempfile().unwrap();
    job.filter = Some(clip_media_protocol::AudioFilter::Graph {
        spec: "amovie=/etc/passwd[out]".into(),
        output: "out".into(),
    });
    let result = command(
        &[&input],
        "render-audio",
        &serde_json::to_string(&job).unwrap(),
    )
    .stdout(output)
    .output()
    .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Unsupported audio filter: amovie"));
}

#[test]
fn executable_streams_only_bounded_raw_pcm_windows() {
    let input = constant_wav(16000, [1234, -2468]);
    let mut job = render_settings(1);
    job.format = "s16le".into();
    job.range = Some((1.25, 1.5));
    job.sample_rate = Some(16000);
    job.channels = Some(1);
    job.filter = Some(clip_media_protocol::AudioFilter::Chain(
        "pan=mono|c0=c1,volume=0.5".into(),
    ));
    let result = command(
        &[&input],
        "render-audio",
        &serde_json::to_string(&job).unwrap(),
    )
    .output()
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout.len(), 8000);
    assert!(
        result
            .stdout
            .as_chunks::<2>()
            .0
            .iter()
            .all(|sample| (i32::from(i16::from_le_bytes(*sample)) + 1234).abs() <= 2)
    );
    job.range = None;
    let result = command(
        &[&input],
        "render-audio",
        &serde_json::to_string(&job).unwrap(),
    )
    .output()
    .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("bounded range"));
}

#[test]
#[ignore = "requires FFmpeg CLI only to generate and reference compressed fixtures"]
fn library_ranges_match_cli_priming_and_container_timing() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    let mut writer = hound::WavWriter::create(
        &source,
        hound::WavSpec {
            channels: 2,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for index in 0..88200 {
        let value = ((index as f64 * 0.063).sin() * 6000.0) as i16;
        writer.write_sample(value).unwrap();
        writer.write_sample(-value).unwrap();
    }
    writer.finalize().unwrap();
    for (extension, codec) in [
        ("flac", "flac"),
        ("mp3", "libmp3lame"),
        ("m4a", "aac"),
        ("ogg", "libvorbis"),
        ("opus", "libopus"),
    ] {
        let path = dir.path().join(format!("source.{extension}"));
        assert!(
            Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&source)
                .args(["-c:a", codec])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        for (first, last) in [(0.025, 0.075), (1.25, 1.5)] {
            let mut job = render_settings(1);
            job.inputs[0].formats = "flac,mp3,mov,ogg".into();
            job.range = Some((first, last));
            job.sample_rate = Some(44100);
            let (_, actual) = wav_samples(render(&[&File::open(&path).unwrap()], &job));
            // Compare with a full-decode trim. CLI input seeking near MP3's
            // priming interval can emit decoder warm-up silence instead.
            let reference = Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&path)
                .args([
                    "-af",
                    &format!("atrim=start={first}:end={last},asetpts=PTS-STARTPTS"),
                    "-ar",
                    "44100",
                    "-c:a",
                    "pcm_s16le",
                    "-f",
                    "s16le",
                    "pipe:1",
                ])
                .output()
                .unwrap();
            assert!(reference.status.success());
            let expected: Vec<_> = reference
                .stdout
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect();
            assert_eq!(actual.len(), expected.len(), "{extension} at {first}");
            let max = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
                .max()
                .unwrap();
            // Independent seeks can restart perceptual-noise synthesis. Reject
            // timing shifts while allowing a small fraction of full scale.
            assert!(
                max < 40,
                "{extension} at {first}: maximum PCM difference {max}"
            );
        }
    }
}

#[test]
fn executable_selects_video_and_cover_frames_from_the_same_pinned_input() {
    use clip_media_protocol::{VideoFrame, VideoStream, parse_video_frame};
    let mut input = tempfile::tempfile().unwrap();
    input
        .write_all(include_bytes!(
            "../../little-clip-core/tests/fixtures/video/with-cover.mp4"
        ))
        .unwrap();
    for (stream, dimensions) in [
        (VideoStream::FirstNonCover, (160, 90)),
        (VideoStream::Ordinal(1), (160, 160)),
    ] {
        let mut job = VideoFrame::new("mov", 160, 160);
        job.stream = stream;
        let result = command(
            &[&input],
            "video-frame",
            &serde_json::to_string(&job).unwrap(),
        )
        .output()
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let frame = parse_video_frame(&result.stdout, 160, 160)
            .unwrap()
            .unwrap();
        assert_eq!((frame.info.width, frame.info.height), dimensions);
        assert_eq!(
            u32::from_be_bytes(frame.png[16..20].try_into().unwrap()),
            dimensions.0
        );
        assert_eq!(
            u32::from_be_bytes(frame.png[20..24].try_into().unwrap()),
            dimensions.1
        );
        assert_eq!(frame.info.seconds, 0.0);
    }
}

#[test]
#[ignore = "requires FFmpeg CLI to generate and independently decode rotated video fixtures"]
fn library_frame_rotation_matches_reference_pixels() {
    use clip_media_protocol::{VideoFrame, parse_video_frame};
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.mp4");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=80x40:rate=5:duration=1",
                "-c:v",
                "mpeg4"
            ])
            .arg(&original)
            .status()
            .unwrap()
            .success()
    );
    for rotation in [0, 90, 180, 270] {
        let path = dir.path().join(format!("rotation-{rotation}.mp4"));
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-display_rotation",
                    &rotation.to_string(),
                    "-i"
                ])
                .arg(&original)
                .args(["-c", "copy"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let settings = VideoFrame::new("mov", 160, 160);
        let result = command(
            &[&File::open(&path).unwrap()],
            "video-frame",
            &serde_json::to_string(&settings).unwrap(),
        )
        .output()
        .unwrap();
        assert!(
            result.status.success(),
            "{rotation}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let frame = parse_video_frame(&result.stdout, 160, 160)
            .unwrap()
            .unwrap();
        let reference = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args([
                "-vf",
                "scale=160:160:force_original_aspect_ratio=decrease",
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(reference.status.success());
        let decoded = reference_rgba(frame.png);
        let expected = if rotation == 90 || rotation == 270 {
            (80, 160)
        } else {
            (160, 80)
        };
        assert_eq!((frame.info.width, frame.info.height), expected);
        assert_eq!(decoded.len(), reference.stdout.len());
        // Rotating subsampled chroma before RGB conversion differs slightly
        // from the CLI conversion path; the bound still rejects wrong rotation.
        let error = decoded
            .iter()
            .zip(&reference.stdout)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum::<u64>() as f64
            / decoded.len() as f64;
        assert!(
            error < 2.0,
            "snapshot rotation {rotation}: mean RGBA error {error}"
        );
        assert_eq!(frame.info.seconds, 0.0);
        let (playing, pixels) = playback::first_frame(&path);
        let expected = if rotation == 90 || rotation == 270 {
            (40, 80)
        } else {
            (80, 40)
        };
        assert_eq!(
            (playing.width, playing.height),
            expected,
            "playback rotation {rotation}"
        );
        let reference = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args([
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(reference.status.success());
        assert_eq!(pixels.len(), reference.stdout.len());
        let mean_error = pixels
            .iter()
            .zip(&reference.stdout)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum::<u64>() as f64
            / pixels.len() as f64;
        assert!(
            mean_error < 2.0,
            "playback rotation {rotation}: mean RGBA error {mean_error}"
        );
    }
}

#[test]
fn executable_preview_samples_variable_holds_and_respects_its_frame_limit() {
    use clip_media_protocol::{VideoFrame, VideoPreview};
    let mut input = tempfile::tempfile().unwrap();
    input
        .write_all(include_bytes!(
            "../../little-clip-core/tests/fixtures/video/timed-colors.gif"
        ))
        .unwrap();
    for (limit, expected) in [(25, 8), (3, 3)] {
        let settings = VideoPreview {
            frame: VideoFrame::new("gif", 80, 80),
            fps: 8,
            frames: limit,
            duration: None,
        };
        let result = command(
            &[&input],
            "video-preview",
            &serde_json::to_string(&settings).unwrap(),
        )
        .output()
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            result
                .stdout
                .windows(2)
                .filter(|bytes| *bytes == [0xff, 0xd8])
                .count(),
            expected
        );
        assert!(result.stdout.ends_with(&[0xff, 0xd9]));
    }
}

#[test]
fn executable_timeline_reduces_each_column_at_the_requested_resolution() {
    use clip_media_protocol::{VideoFrame, VideoTimeline};
    let mut input = tempfile::tempfile().unwrap();
    input
        .write_all(include_bytes!(
            "../../little-clip-core/tests/fixtures/video/timed-colors.gif"
        ))
        .unwrap();
    let settings = VideoTimeline {
        frame: VideoFrame::new("gif", 1, 64),
        columns: 16,
        duration_ms: 1000,
    };
    let result = command(
        &[&input],
        "video-timeline",
        &serde_json::to_string(&settings).unwrap(),
    )
    .output()
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.starts_with(&[0xff, 0xd8]));
    assert!(result.stdout.ends_with(&[0xff, 0xd9]));
}

fn reference_rgba(bytes: &[u8]) -> Vec<u8> {
    use std::process::Stdio;
    let mut decode = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "image2pipe",
            "-i",
            "pipe:0",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    decode.stdin.take().unwrap().write_all(bytes).unwrap();
    let decoded = decode.wait_with_output().unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    decoded.stdout
}

#[test]
#[ignore = "requires FFmpeg CLI to generate and independently compare an AV1 keyframe timeline"]
fn library_av1_timeline_matches_the_existing_sequential_reference() {
    use clip_media_protocol::{VideoFrame, VideoTimeline};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyframes.mkv");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=80x40:rate=10:duration=3",
                "-c:v",
                "libaom-av1",
                "-cpu-used",
                "8",
                "-g",
                "10",
                "-threads",
                "2"
            ])
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let settings = VideoTimeline {
        frame: VideoFrame::new("matroska", 1, 16),
        columns: 48,
        duration_ms: 3000,
    };
    let result = command(
        &[&File::open(&path).unwrap()],
        "video-timeline",
        &serde_json::to_string(&settings).unwrap(),
    )
    .output()
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let reference = Command::new("ffmpeg")
        .args(["-v", "error", "-skip_frame", "nokey", "-threads", "2", "-i"])
        .arg(&path)
        .args([
            "-vf",
            "scale=1:16:flags=area,tpad=stop_mode=clone:stop_duration=3,fps=48000/3000,tile=48x1",
            "-frames:v",
            "1",
            "-q:v",
            "2",
            "-pix_fmt",
            "yuvj444p",
            "-f",
            "image2pipe",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        reference.status.success(),
        "{}",
        String::from_utf8_lossy(&reference.stderr)
    );
    let actual = reference_rgba(&result.stdout);
    let expected = reference_rgba(&reference.stdout);
    assert_eq!(actual.len(), 48 * 16 * 4);
    assert_eq!(actual.len(), expected.len());
    let maximum = actual
        .iter()
        .zip(&expected)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert_eq!(maximum, 0, "AV1 timeline reference pixel difference");
}

mod playback {
    use super::*;
    use clip_media_protocol::playback::{Control, Settings, Update};
    use std::{
        io::Read,
        process::{Child, ChildStdin, ChildStdout, Stdio},
        time::{Duration, Instant},
    };

    struct Player {
        child: Child,
        input: Option<ChildStdin>,
        output: ChildStdout,
    }
    impl Player {
        fn open() -> Self {
            let settings = Self::settings(1200);
            Self::source(
                std::path::Path::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/video/playback-colors.mp4"
                )),
                settings,
            )
        }
        fn settings(start_ms: u64) -> Settings {
            Settings {
                stream: 0,
                width: 320,
                height: 180,
                fps: 20,
                start_ms,
                duration_ms: 3000,
                silent: true,
                options: Default::default(),
            }
        }
        fn source(path: &std::path::Path, settings: Settings) -> Self {
            let input = File::open(path).unwrap();
            let mut child = command(
                &[&input],
                "playback",
                &serde_json::to_string(&settings).unwrap(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
            Self {
                input: child.stdin.take(),
                output: child.stdout.take().unwrap(),
                child,
            }
        }
        fn control(&mut self, epoch: u64, seek_ms: Option<u64>, playing: bool) {
            let bytes = serde_json::to_string(&Control {
                epoch,
                seek_ms,
                playing,
                volume: 0.5,
                options: Default::default(),
            })
            .unwrap();
            writeln!(self.input.as_mut().unwrap(), "{bytes}").unwrap();
        }
        fn until(&mut self, accept: impl Fn(&Update) -> bool) -> (Update, Vec<u8>) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let packet = Update::read(&mut Timed {
                    output: &mut self.output,
                    deadline,
                })
                .unwrap();
                assert!(packet.0.error.is_none(), "{:?}", packet.0);
                if accept(&packet.0) {
                    return packet;
                }
            }
        }
    }
    impl Drop for Player {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    struct Timed<'a> {
        output: &'a mut ChildStdout,
        deadline: Instant,
    }
    impl Read for Timed<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            loop {
                if Instant::now() >= self.deadline {
                    return Err(std::io::Error::other("Playback test timed out"));
                }
                let mut fd = libc::pollfd {
                    fd: self.output.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: poll borrows the test's live child stdout descriptor.
                let ready = unsafe { libc::poll(&mut fd, 1, 25) };
                if ready < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if ready > 0 {
                    return self.output.read(bytes);
                }
            }
        }
    }
    pub(super) fn first_frame(path: &std::path::Path) -> (Update, Vec<u8>) {
        let mut player = Player::source(
            path,
            Settings {
                stream: 0,
                width: 160,
                height: 160,
                fps: 20,
                start_ms: 0,
                duration_ms: 1000,
                silent: true,
                options: Default::default(),
            },
        );
        player.until(|packet| packet.bytes > 0)
    }
    #[test]
    fn embedded_tracks_render_subtitles_and_options_seek_at_the_requested_rate_and_loop() {
        use clip_media_protocol::playback::Options;
        let mut player = Player::source(
            std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../little-clip-core/tests/fixtures/video/playback-tracks.mkv"
            )),
            Player::settings(1000),
        );
        let (initial, dark) = player.until(|p| p.bytes > 0 && p.tracks.is_some());
        let tracks = initial.tracks.unwrap();
        assert_eq!(tracks.audio.len(), 2);
        assert_eq!(tracks.subtitles.len(), 2);
        assert!(tracks.subtitles[0].label.contains("en"), "{tracks:?}");
        let mut options = Options {
            subtitles_enabled: true,
            subtitle_track: Some(0),
            audio_track: Some(1),
            ..Options::default()
        };
        let send = |player: &mut Player, epoch, seek_ms, playing, options: &Options| {
            writeln!(
                player.input.as_mut().unwrap(),
                "{}",
                serde_json::to_string(&Control {
                    epoch,
                    seek_ms: Some(seek_ms),
                    playing,
                    volume: 1.0,
                    options: options.clone()
                })
                .unwrap()
            )
            .unwrap();
        };
        send(&mut player, 1, 1000, false, &options);
        let (selected, subtitled) = player.until(|p| p.epoch == 1 && p.bytes > 0);
        assert_eq!(selected.tracks.unwrap().audio.len(), 2);
        let bright = |pixels: &[u8]| {
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[0] > 120 && p[1] > 120 && p[2] > 120)
                .count()
        };
        assert!(
            bright(&subtitled) > bright(&dark) + 100,
            "subtitles must change actual video pixels"
        );
        options.subtitle_track = Some(1);
        send(&mut player, 2, 1000, false, &options);
        let (_, second) = player.until(|p| p.epoch == 2 && p.bytes > 0);
        assert_ne!(
            subtitled, second,
            "switching subtitle track changes rendered text"
        );
        options.subtitles_enabled = false;
        options.rate = 4.0;
        options.looping = true;
        send(&mut player, 3, 1000, true, &options);
        let (_, off) = player.until(|p| p.epoch == 3 && p.bytes > 0);
        assert!(bright(&off) <= bright(&dark) + 5);
        let start = Instant::now();
        player.until(|p| p.epoch == 3 && p.position_ms >= 2400 && p.bytes > 0);
        assert!(
            start.elapsed() < Duration::from_millis(1000),
            "4x playback should advance 1.4 seconds in substantially less than real time"
        );
        player.until(|p| p.epoch == 3 && p.position_ms < 500 && p.bytes > 0);
    }

    #[test]
    fn player_clocks_frames_pauses_seeks_with_epochs_and_reports_eof_without_a_window() {
        let mut player = Player::open();
        let (start, pixels) = player.until(|packet| packet.bytes > 0);
        assert!((1200..1400).contains(&start.position_ms), "{start:?}");
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[1] > 80 && pixel[0] < 10 && pixel[2] < 10)
        );
        let clock = Instant::now();
        player.until(|packet| packet.position_ms >= 1600 && packet.bytes > 0);
        assert!(
            clock.elapsed() >= Duration::from_millis(250),
            "frames must be clocked, not decoded as fast as possible"
        );
        player.control(0, None, false);
        let (paused, _) = player.until(|packet| !packet.playing && packet.bytes == 0);
        let (still, _) = player.until(|packet| !packet.playing && packet.bytes == 0);
        assert!(paused.position_ms.abs_diff(still.position_ms) <= 20);
        player.control(1, Some(2200), false);
        let (seek, pixels) = player.until(|packet| packet.epoch == 1 && packet.bytes > 0);
        assert!((2200..2300).contains(&seek.position_ms), "{seek:?}");
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[2] > 200 && pixel[0] < 10)
        );
        player.control(2, Some(200), true);
        let (_, pixels) = player.until(|packet| packet.epoch == 2 && packet.bytes > 0);
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[0] > 200 && pixel[2] < 10)
        );
        let (end, _) = player.until(|packet| packet.epoch == 2 && packet.ended);
        assert!(!end.playing);
        assert_eq!(end.duration_ms, 3000);
        drop(player.input.take());
        let deadline = Instant::now() + Duration::from_secs(3);
        while player.child.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "closing controls must retire playback"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
