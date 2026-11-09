//! Guarded seekable sources, with bounded PCM preparation for bridged formats.
mod ncw;
use super::AudioMetadata;
use crate::process::CommandRequest;
use std::{
    collections::VecDeque,
    fs::File,
    io::{Cursor, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use symphonia::core::io::MediaSource;
const MAX_PCM: usize = 256 * 1024 * 1024;
const ASF: &[u8] = b"\x30\x26\xb2\x75\x8e\x66\xcf\x11\xa6\xd9\x00\xaa\x00\x62\xce\x6c";
const ASF_INPUT_ARGS: &[&str] = &[
    "-f",
    "asf",
    "-protocol_whitelist",
    "file,pipe,fd",
    "-max_alloc",
    "67108864",
];
const XWMA_INPUT_ARGS: &[&str] = &[
    "-f",
    "xwma",
    "-protocol_whitelist",
    "file,pipe,fd",
    "-max_alloc",
    "67108864",
];
fn container_args(header: &[u8]) -> Option<&'static [&'static str]> {
    if header == ASF {
        Some(ASF_INPUT_ARGS)
    } else if header.get(..4) == Some(b"RIFF") && header.get(8..12) == Some(b"XWMA") {
        Some(XWMA_INPUT_ARGS)
    } else {
        None
    }
}
/// Selects local-protocol demuxers for `.wma` and `.xwma` tool inputs.
/// Callers still authorize content with `CommandRequest::arg_content_file`.
pub fn decoder_input_args(path: &Path) -> &'static [&'static str] {
    let Some(extension) = path.extension() else {
        return &[];
    };
    if extension.eq_ignore_ascii_case("wma") {
        ASF_INPUT_ARGS
    } else if extension.eq_ignore_ascii_case("xwma") || extension.eq_ignore_ascii_case("xwm") {
        XWMA_INPUT_ARGS
    } else {
        &[]
    }
}
/// Identifies formats whose metadata/PCM preparation belongs on a worker.
/// This filename hint does not replace content identification on the pinned input.
pub fn needs_audio_preparation(path: &Path) -> bool {
    !decoder_input_args(path).is_empty()
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ncw"))
}
/// Seekable encoded or prepared audio, suitable for Symphonia and Rodio.
pub type SeekableAudio = Box<dyn MediaSource>;
/// One owned source and optional shared playback snapshot.
pub struct AudioInput {
    /// Independent-position reader over ordinary content or prepared WAV bytes.
    pub stream: SeekableAudio,
    /// A complete float PCM WAV snapshot when native decoding needs a bridge.
    pub decoded: Option<Arc<[u8]>>,
}
/// Owned input for tools that cannot decode every native application format.
pub struct ToolAudioInput {
    /// Ordinary source or an unlinked, seekable prepared snapshot.
    pub file: File,
    /// Whether the tool should select its WAV demuxer for this snapshot.
    pub prepared_wav: bool,
}
/// Opens one pinned input and materializes prepared PCM only when needed.
///
/// # Errors
/// Returns source/preparation errors or a failure to write the bounded temporary
/// snapshot. The caller must pass the owned handle to the child, not a temp path.
pub fn open_tool_audio_input(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<ToolAudioInput, String> {
    let file = crate::kernel_interface::open_content(path, true).map_err(|e| e.to_string())?;
    let input = from_file(file.try_clone().map_err(|e| e.to_string())?, cancelled)?;
    if let Some(bytes) = input.decoded {
        let mut snapshot = tempfile::tempfile().map_err(|e| e.to_string())?;
        snapshot.write_all(&bytes).map_err(|e| e.to_string())?;
        snapshot
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        Ok(ToolAudioInput {
            file: snapshot,
            prepared_wav: true,
        })
    } else if needs_audio_preparation(path) {
        Err("Audio input header does not match its format".into())
    } else {
        Ok(ToolAudioInput {
            file,
            prepared_wav: false,
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Revision {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
fn revision(file: &File) -> Result<Revision, String> {
    let m = file.metadata().map_err(|e| e.to_string())?;
    Ok(Revision {
        device: m.dev(),
        inode: m.ino(),
        size: m.len(),
        modified: (m.mtime(), m.mtime_nsec()),
        changed: (m.ctime(), m.ctime_nsec()),
    })
}
struct Cached {
    revision: Revision,
    source: File,
    bytes: Arc<[u8]>,
}
fn cache() -> &'static Mutex<VecDeque<Cached>> {
    static CACHE: OnceLock<Mutex<VecDeque<Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}
fn cached(key: Revision) -> Option<Arc<[u8]>> {
    let mut cache = cache().lock().ok()?;
    let index = cache
        .iter()
        .position(|entry| entry.revision == key && revision(&entry.source).ok() == Some(key))?;
    let entry = cache.remove(index)?;
    let bytes = entry.bytes.clone();
    cache.push_back(entry);
    Some(bytes)
}
fn remember(key: Revision, source: File, bytes: Arc<[u8]>) {
    if let Ok(mut cache) = cache().lock() {
        cache.retain(|entry| entry.revision != key);
        while cache.len() >= 2
            || cache.iter().map(|entry| entry.bytes.len()).sum::<usize>() + bytes.len()
                > MAX_PCM + 112
        {
            if cache.pop_front().is_none() {
                break;
            }
        }
        cache.push_back(Cached {
            revision: key,
            source,
            bytes,
        });
    }
}
/// Opens ordinary audio and prepares Windows Media/NCW into a bounded, seekable PCM snapshot.
/// This may run FFmpeg and belongs on a bounded worker, never the UI thread.
///
/// # Errors
/// Rejects protected/nonregular sources, changed identity, cancellation, decoder failure
/// and output exceeding the 256 MiB decoded-audio budget.
pub fn open_audio_input(path: &Path, cancelled: &AtomicBool) -> Result<AudioInput, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Audio opening cancelled".into());
    }
    let file = crate::kernel_interface::open_content(path, true).map_err(|e| e.to_string())?;
    from_file(file, cancelled)
}
/// Prepares audio from one retained ordinary file handle.
///
/// # Errors
/// Rejects protected/nonregular handles, cancellation, changed inputs and decoder budgets.
pub fn from_file(mut file: File, cancelled: &AtomicBool) -> Result<AudioInput, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Audio opening cancelled".into());
    }
    if crate::kernel_interface::classify_handle(&file)
        .map_err(|e| e.to_string())?
        .protected()
        || !file.metadata().map_err(|e| e.to_string())?.is_file()
    {
        return Err("Audio source is not ordinary file content".into());
    }
    let mut header = [0u8; 16];
    let count = file.read_at(&mut header, 0).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let native_ncw = ncw::recognizes(&header[..count]);
    if !native_ncw && container_args(&header[..count]).is_none() {
        return Ok(AudioInput {
            stream: Box::new(file),
            decoded: None,
        });
    }
    let bytes = prepared_snapshot(file, native_ncw, cancelled)?;
    Ok(AudioInput {
        stream: Box::new(Cursor::new(bytes.clone())),
        decoded: Some(bytes),
    })
}
// One cache lifecycle for both native and process-based preparation. Keep the
// original handle until its decoded snapshot has passed the identity check.
fn prepared_snapshot(
    file: File,
    native_ncw: bool,
    cancelled: &AtomicBool,
) -> Result<Arc<[u8]>, String> {
    let key = revision(&file)?;
    if key.size > 512 * 1024 * 1024 {
        return Err("Audio source exceeds the 512 MiB input budget".into());
    }
    let bytes = if let Some(bytes) = cached(key) {
        bytes
    } else {
        let bytes: Arc<[u8]> = if native_ncw {
            ncw::prepare(&file, cancelled)?
        } else {
            prepare(&file, cancelled)?
        }
        .into();
        if revision(&file)? != key {
            return Err("Audio source changed while decoding".into());
        }
        remember(key, file, bytes.clone());
        bytes
    };
    Ok(bytes)
}

struct Probe {
    rate: u32,
    channels: u16,
    duration: Option<f64>,
    input_args: &'static [&'static str],
}
fn probe(file: &File, cancelled: &AtomicBool) -> Result<Probe, String> {
    let mut magic = [0u8; 16];
    let count = file.read_at(&mut magic, 0).map_err(|e| e.to_string())?;
    let input_args = container_args(&magic[..count]).ok_or("Not an ASF/xWMA Windows Media file")?;

    let path = format!("/proc/self/fd/{}", file.as_raw_fd());
    let output = CommandRequest::new("ffprobe")
        .args(input_args)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=sample_rate,channels:format=duration",
            "-of",
            "json",
        ])
        .arg_content_file(&path)
        .map_err(|e| e.to_string())?
        .timeout(Duration::from_secs(3))
        .max_output_bytes(64 * 1024)
        .run_cancellable(cancelled)
        .map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err("Could not probe the Windows Media audio stream".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let stream = &value["streams"][0];
    let rate = stream["sample_rate"]
        .as_str()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|v| (1..=384000).contains(v))
        .ok_or("Unsupported audio sample rate")?;
    let channels = stream["channels"]
        .as_u64()
        .filter(|n| (1..=8).contains(n))
        .ok_or("Unsupported audio channel count")? as u16;
    let duration = value["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0);
    Ok(Probe {
        rate,
        channels,
        duration,
        input_args,
    })
}
/// Reads ASF/xWMA/NCW metadata without developing the entire PCM source.
///
/// # Errors
/// Returns guarded opening, probing, cancellation or unsupported-stream errors.
pub(super) fn read_prepared_audio_metadata(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<AudioMetadata, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Audio metadata opening cancelled".into());
    }
    let file = crate::kernel_interface::open_content(path, true).map_err(|e| e.to_string())?;
    let mut magic = [0u8; 16];
    let count = file.read_at(&mut magic, 0).map_err(|e| e.to_string())?;
    if ncw::recognizes(&magic[..count]) {
        return ncw::metadata(&file);
    }
    let info = probe(&file, cancelled)?;
    Ok(AudioMetadata {
        duration_ms: (info.duration.ok_or("Audio duration is unavailable")? * 1000.0) as u64,
        sample_rate: info.rate,
        channels: info.channels,
        format: if info.input_args == XWMA_INPUT_ARGS {
            "xWMA"
        } else {
            "WMA"
        }
        .into(),
    })
}
fn prepare(file: &File, cancelled: &AtomicBool) -> Result<Vec<u8>, String> {
    let info = probe(file, cancelled)?;
    let path = format!("/proc/self/fd/{}", file.as_raw_fd());
    let output = CommandRequest::new("ffmpeg")
        .args(info.input_args)
        .args(["-v", "error", "-nostdin", "-xerror", "-threads", "1", "-i"])
        .arg_content_file(&path)
        .map_err(|e| e.to_string())?
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-acodec",
            "pcm_f32le",
            "-ar",
            &info.rate.to_string(),
            "-ac",
            &info.channels.to_string(),
            "-f",
            "f32le",
            "pipe:1",
        ])
        .timeout(Duration::from_secs(30))
        .max_output_bytes(MAX_PCM)
        .run_cancellable(cancelled)
        .map_err(|e| e.to_string())?;
    if output.stdout_truncated {
        return Err("Decoded audio exceeds the 256 MiB source budget".into());
    }
    if !output.status.success() {
        return Err(format!(
            "Windows Media decoding failed: {}",
            output.stderr_lossy().chars().take(512).collect::<String>()
        ));
    }
    pcm_wav(output.stdout, info.rate, info.channels)
}
fn pcm_wav(mut pcm: Vec<u8>, rate: u32, channels: u16) -> Result<Vec<u8>, String> {
    let align = channels * 4;
    if pcm.is_empty() || pcm.len() % (align as usize) != 0 || pcm.len() > MAX_PCM {
        return Err("Invalid or oversized float PCM output".into());
    }
    let length = pcm.len() as u32;
    let mut header = Vec::with_capacity(56);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(length + 48).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&3u16.to_le_bytes());
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&rate.to_le_bytes());
    header.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
    header.extend_from_slice(&align.to_le_bytes());
    header.extend_from_slice(&32u16.to_le_bytes());
    header.extend_from_slice(b"fact");
    header.extend_from_slice(&4u32.to_le_bytes());
    header.extend_from_slice(&(length / u32::from(align)).to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&length.to_le_bytes());
    pcm.splice(0..0, header);
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    static TEST_CACHE: Mutex<()> = Mutex::new(());
    fn path(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/audio")
            .join(name)
    }
    #[test]
    fn native_ncw_uses_the_shared_snapshot_cache_from_any_descriptor_position() {
        let _cache = TEST_CACHE.lock().unwrap();
        let values = vec![16384i32; 22050 * 2];
        let bytes = ::ncw::encode_pcm(
            &values,
            ::ncw::PcmSpec {
                channels: 1,
                bits_per_sample: 16,
                sample_rate: 22050,
            },
            ::ncw::StereoMode::Direct,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.NCW");
        let mut file = std::fs::File::create_new(&path).unwrap();
        file.write_all(&bytes).unwrap();
        drop(file);
        let mut file = std::fs::File::open(&path).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        let first = from_file(file, &AtomicBool::new(false)).unwrap();
        let second = open_audio_input(&path, &AtomicBool::new(false)).unwrap();
        assert!(Arc::ptr_eq(
            first.decoded.as_ref().unwrap(),
            second.decoded.as_ref().unwrap()
        ));
        let metadata = super::super::read_audio_metadata(&path).unwrap();
        assert_eq!(metadata.format, "NCW");
        assert_eq!(metadata.duration_ms, 2000);
        assert!(super::super::is_audio_path(&path));
        assert!(needs_audio_preparation(&path));
        assert!(from_file(std::fs::File::open(&path).unwrap(), &AtomicBool::new(true)).is_err());
    }
    #[test]
    fn float_wav_snapshots_are_seekable_shared_and_keep_channel_metadata() {
        let _cache = TEST_CACHE.lock().unwrap();
        let cancel = AtomicBool::new(false);
        for (name, channels, rate) in [
            ("tones.wma", 2, 44100),
            ("mono.wma", 1, 22050),
            ("tones.xwma", 2, 44100),
            ("mono.xwma", 1, 22050),
        ] {
            let path = path(name);
            let original = std::fs::read(&path).unwrap();
            let metadata = read_prepared_audio_metadata(&path, &cancel).unwrap();
            assert_eq!((metadata.channels, metadata.sample_rate), (channels, rate));
            let mut first = open_audio_input(&path, &cancel).unwrap();
            let mut second = open_audio_input(&path, &cancel).unwrap();
            assert!(Arc::ptr_eq(
                first.decoded.as_ref().unwrap(),
                second.decoded.as_ref().unwrap()
            ));
            first.stream.seek(SeekFrom::Start(80)).unwrap();
            let mut magic = [0u8; 4];
            second.stream.read_exact(&mut magic).unwrap();
            assert_eq!(&magic, b"RIFF");
            first.stream.seek(SeekFrom::Start(0)).unwrap();
            let format = symphonia::default::get_probe()
                .format(
                    &symphonia::core::probe::Hint::new(),
                    symphonia::core::io::MediaSourceStream::new(first.stream, Default::default()),
                    &Default::default(),
                    &Default::default(),
                )
                .unwrap()
                .format;
            let params = &format.default_track().unwrap().codec_params;
            assert_eq!(params.sample_rate, Some(rate));
            assert_eq!(params.channels.unwrap().count(), channels as usize);
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }
    #[test]
    fn replaced_sources_cannot_reuse_the_previous_pcm_snapshot() {
        let _cache = TEST_CACHE.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("same.wma");
        let cancel = AtomicBool::new(false);
        std::fs::copy(path("tones.wma"), &file).unwrap();
        let first = open_audio_input(&file, &cancel).unwrap();
        std::fs::rename(&file, directory.path().join("old.wma")).unwrap();
        std::fs::copy(path("mono.wma"), &file).unwrap();
        let second = open_audio_input(&file, &cancel).unwrap();
        assert!(!Arc::ptr_eq(
            first.decoded.as_ref().unwrap(),
            second.decoded.as_ref().unwrap()
        ));
        assert!(open_audio_input(&file, &AtomicBool::new(true)).is_err());
        assert!(open_audio_input(Path::new("/proc/cpuinfo"), &cancel).is_err());
    }
    #[test]
    fn oversized_inputs_and_disguised_playlists_do_not_enter_a_decoder() {
        let directory = tempfile::tempdir().unwrap();
        let large = directory.path().join("large.wma");
        let mut file = File::create(&large).unwrap();
        std::io::Write::write_all(&mut file, ASF).unwrap();
        file.set_len(512 * 1024 * 1024 + 1).unwrap();
        assert!(
            open_audio_input(&large, &AtomicBool::new(false))
                .err()
                .unwrap()
                .contains("512 MiB")
        );
        let playlist = directory.path().join("playlist.wma");
        std::fs::write(&playlist, b"#EXTM3U\nhttp://example.invalid/music\n").unwrap();
        assert!(read_prepared_audio_metadata(&playlist, &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn xwma_signature_selects_a_restricted_demuxer_and_enters_shared_analysis() {
        assert!(super::super::is_audio_path(Path::new("GAME.XWM")));
        assert_eq!(decoder_input_args(Path::new("GAME.XWMA")), XWMA_INPUT_ARGS);
        assert_eq!(decoder_input_args(Path::new("GAME.XWM")), XWMA_INPUT_ARGS);
        assert!(container_args(b"RIFF0000WAVE0000").is_none());
        let analysis =
            super::super::compute_audio_analysis_progressive(&path("tones.xwma"), 120, |_| {})
                .unwrap();
        assert!(analysis.decoded_source.is_some());
        assert_eq!((analysis.channels, analysis.sample_rate), (2, 44100));
        assert!((3900..4300).contains(&analysis.duration_ms));
        assert!(analysis.ch1_samples.iter().any(|value| value.abs() > 0.2));
        let directory = tempfile::tempdir().unwrap();
        let playlist = directory.path().join("fake.xwma");
        std::fs::write(&playlist, b"#EXTM3U\nhttp://example.invalid/audio\n").unwrap();
        assert!(read_prepared_audio_metadata(&playlist, &AtomicBool::new(false)).is_err());
    }
    #[test]
    #[ignore = "set LITTLE_CLIP_XWMA_REFERENCE to an ordinary RIFF xWMA reference file"]
    fn standard_xwma_without_codec_extradata_uses_the_same_guarded_bridge() {
        let path = std::path::PathBuf::from(
            std::env::var_os("LITTLE_CLIP_XWMA_REFERENCE").expect("reference file path"),
        );
        let info = read_prepared_audio_metadata(&path, &AtomicBool::new(false)).unwrap();
        assert!(info.duration_ms > 0);
        assert_eq!(info.format, "xWMA");
        let source = open_audio_input(&path, &AtomicBool::new(false)).unwrap();
        assert!(source.decoded.unwrap().len() > 1000);
    }
    #[test]
    fn prepared_audio_flows_through_full_analysis_and_preview_waveforms() {
        let _cache = TEST_CACHE.lock().unwrap();
        let path = path("tones.wma");
        let analysis =
            super::super::compute_audio_analysis_progressive(&path, 120, |_| {}).unwrap();
        assert_eq!((analysis.channels, analysis.sample_rate), (2, 44100));
        assert!(analysis.decoded_source.is_some());
        assert!((3900..4300).contains(&analysis.duration_ms));
        assert!(analysis.ch1_samples.iter().any(|value| value.abs() > 0.2));
        let file = crate::kernel_interface::open_content(&path, true).unwrap();
        let (peaks, colors) = super::super::compute_preview_waveform(
            file,
            &path,
            4.0,
            100,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(peaks.len(), 100);
        assert_eq!(colors.len(), 100);
        assert!(peaks.iter().any(|p| *p > 0.2));
    }
}
