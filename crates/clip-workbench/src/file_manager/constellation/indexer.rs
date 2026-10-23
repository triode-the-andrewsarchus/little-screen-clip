//! Discovers, analyzes, and commits complete recursive sample index snapshots.
//!
//! Discovery never follows symlink directories and visits ordinary directories
//! once per device/inode identity, preventing bind-alias cycles. Hidden directories
//! and generated sample packs are excluded. Entry errors fail the snapshot instead
//! of publishing an incomplete replacement. Unsupported/silent audio is omitted by
//! the classifier; this differs from an incomplete directory inventory.
//!
//! The analysis pool uses at most eight workers and reserves one reported CPU
//! when possible. Progress counts completed analyses, including unclassified files.
//! Each analysis decodes at most three seconds while preserving full decoder
//! duration for the HUD and pack metadata when available. Indexing and slot caches
//! share header limits (384 kHz, eight channels) before deriving allocation sizes.

use super::classifier::classify_sample;
use super::db::{ConstellationDb, ConstellationSample};
use rodio::{Decoder, Source};
use std::collections::VecDeque;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

#[derive(Clone, Debug)]
pub enum IndexProgress {
    Scanning {
        files_found: usize,
    },
    Indexing {
        processed: usize,
        total: usize,
        current_file: String,
    },
    Complete {
        samples: Vec<ConstellationSample>,
    },
    Error(String),
}

pub fn is_audio_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some("wav" | "wave" | "mp3" | "flac" | "ogg" | "aif" | "aiff" | "m4a" | "aac") => {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            !name.starts_with('.') && !name.starts_with("._")
        }
        _ => false,
    }
}

/// Discovers and analyzes a complete folder snapshot before replacing its cached index.
///
/// Runs off the UI thread. Cancellation publishes no partial results; discovery,
/// worker, and persistence failures leave the previously committed index intact.
pub fn run_indexing(
    folder: PathBuf,
    cancelled: Arc<AtomicBool>,
    sender: mpsc::Sender<IndexProgress>,
) {
    let result = discover_audio_paths(&folder, &cancelled, &sender)
        .and_then(|paths| analyze_audio_paths(&paths, &folder, &cancelled, &sender));
    if cancelled.load(Ordering::Relaxed) {
        return;
    }
    let samples = match result {
        Ok(samples) => samples,
        Err(error) => {
            let _ = sender.send(IndexProgress::Error(error));
            return;
        }
    };
    if let Err(error) = ConstellationDb::new().and_then(|db| db.save_samples(&folder, &samples)) {
        let _ = sender.send(IndexProgress::Error(format!(
            "Cannot save sample index: {error}"
        )));
        return;
    }
    let _ = sender.send(IndexProgress::Complete { samples });
}

/// Inventories ordinary directories once per filesystem identity, excluding hidden/pack trees.
fn discover_audio_paths(
    folder: &Path,
    cancelled: &AtomicBool,
    sender: &mpsc::Sender<IndexProgress>,
) -> Result<Vec<PathBuf>, String> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    let mut paths = Vec::new();
    let mut directories = VecDeque::from([folder.to_path_buf()]);
    let mut visited = std::collections::HashSet::new();
    while let Some(path) = directories.pop_front() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Indexing cancelled".into());
        }
        let directory = little_clip_core::kernel_interface::pin_content_directory(&path)
            .map_err(|error| format!("Cannot scan {}: {error}", path.display()))?;
        let metadata = directory.metadata().map_err(|error| error.to_string())?;
        if !visited.insert((metadata.dev(), metadata.ino())) {
            continue;
        }
        let entries = std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
            .map_err(|error| error.to_string())?;
        for entry in entries {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Indexing cancelled".into());
            }
            // Failing the snapshot is preferable to silently deleting cached rows
            // for entries whose metadata could not be collected.
            let entry =
                entry.map_err(|error| format!("Cannot enumerate {}: {error}", path.display()))?;
            let name = entry.file_name();
            let display_name = name.to_string_lossy();
            if display_name.starts_with('.')
                || ["__MACOSX", super::sample_pack::PACKS_DIR_NAME].contains(&display_name.as_ref())
            {
                continue;
            }
            let target = path.join(name);
            let kind = entry
                .file_type()
                .map_err(|error| format!("Cannot inspect {}: {error}", target.display()))?;
            if kind.is_dir() {
                directories.push_back(target);
            } else if kind.is_file() && is_audio_file(&target) {
                paths.push(target);
                if paths.len() % 50 == 0 {
                    let _ = sender.send(IndexProgress::Scanning {
                        files_found: paths.len(),
                    });
                }
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// Classifies files on a bounded pool while leaving one available CPU for playback/UI.
fn analyze_audio_paths(
    paths: &[PathBuf],
    folder: &Path,
    cancelled: &AtomicBool,
    sender: &mpsc::Sender<IndexProgress>,
) -> Result<Vec<ConstellationSample>, String> {
    let workers = std::thread::available_parallelism()
        .map(|count| count.get().saturating_sub(1))
        .unwrap_or(2)
        .clamp(1, 8)
        .min(paths.len());
    let next = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| -> Result<Vec<ConstellationSample>, String> {
                    let mut samples = Vec::new();
                    while !cancelled.load(Ordering::Relaxed) && !failed.load(Ordering::Relaxed) {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = paths.get(index) else {
                            break;
                        };
                        let filename = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        if let Some(sample) =
                            process_single_audio_file(path, filename.clone(), folder).inspect_err(
                                |_| {
                                    failed.store(true, Ordering::Relaxed);
                                },
                            )?
                        {
                            samples.push(sample);
                        }
                        let count = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        if count % 10 == 0 || count == paths.len() {
                            let _ = sender.send(IndexProgress::Indexing {
                                processed: count,
                                total: paths.len(),
                                current_file: filename,
                            });
                        }
                    }
                    Ok(samples)
                })
            })
            .collect();
        // Join every handle even if one panics: no unobserved child panic may
        // unwind the scoped pool or turn the remaining results into a partial save.
        let joined: Vec<_> = handles.into_iter().map(|handle| handle.join()).collect();
        let results = joined
            .into_iter()
            .map(|result| {
                result
                    .map_err(|_| {
                        "Sample analysis worker panicked; previous index retained".to_string()
                    })
                    .and_then(|result| result)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(results.into_iter().flatten().collect())
    })
}

fn process_single_audio_file(
    path: &Path,
    filename: String,
    root_folder: &Path,
) -> Result<Option<ConstellationSample>, String> {
    let file = little_clip_core::kernel_interface::open_content(path, true)
        .map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
    let meta = file
        .metadata()
        .map_err(|error| format!("Cannot inspect {}: {error}", path.display()))?;
    let file_size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let Ok(mut decoder) = Decoder::try_from(BufReader::new(file)) else {
        return Ok(None);
    };
    let sample_rate = decoder.sample_rate().get() as f32;
    let channels = decoder.channels().get() as usize;
    if sample_rate > super::MAX_SAMPLE_RATE as f32 || channels > super::MAX_SAMPLE_CHANNELS as usize
    {
        return Ok(None);
    }

    let duration = decoder
        .total_duration()
        .map(|duration| duration.as_secs_f32());

    // Bound acoustic analysis while retaining the full source duration.
    let max_samples_per_ch = (sample_rate * 3.0) as usize;
    let max_total_samples = max_samples_per_ch * channels;

    let mut mono_samples = Vec::with_capacity(max_samples_per_ch);
    let mut ch_idx = 0;
    let mut current_frame_sum = 0.0f32;

    for s in decoder.by_ref().take(max_total_samples) {
        current_frame_sum += s;
        ch_idx += 1;
        if ch_idx >= channels {
            mono_samples.push(current_frame_sum / channels as f32);
            current_frame_sum = 0.0;
            ch_idx = 0;
        }
    }

    if mono_samples.is_empty() {
        return Ok(None);
    }

    let Some(classification) = classify_sample(path, &mono_samples, sample_rate) else {
        return Ok(None);
    };
    let parent_folder = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root_folder.to_path_buf());

    let format_info = super::audio_probe::read_audio_format_info(path);
    let sample_rate_u32 = format_info
        .sample_rate
        .unwrap_or(decoder.sample_rate().get());
    let channels_u16 = format_info.channels.unwrap_or(channels as u16);
    let bit_depth_u16 = format_info.bit_depth.unwrap_or(0);

    Ok(Some(ConstellationSample {
        file_path: path.to_path_buf(),
        folder_path: parent_folder,
        filename,
        category: classification.category,
        confidence: classification.confidence,
        duration_seconds: duration.unwrap_or(classification.duration),
        pitch_hz: classification.pitch_hz,
        pitch_clarity: classification.pitch_clarity,
        brightness: classification.brightness,
        x: classification.x,
        y: classification.y,
        file_size,
        mtime,
        sample_rate: sample_rate_u32,
        channels: channels_u16,
        bit_depth: bit_depth_u16,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_excludes_hidden_pack_and_symlink_trees() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("subfolder");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(root.path().join("kick.wav"), []).unwrap();
        std::fs::write(child.join("snare.WAV"), []).unwrap();
        std::fs::write(root.path().join(".hidden.wav"), []).unwrap();
        let packs = root.path().join(super::super::sample_pack::PACKS_DIR_NAME);
        std::fs::create_dir(&packs).unwrap();
        std::fs::write(packs.join("linked.wav"), []).unwrap();
        std::os::unix::fs::symlink(root.path(), child.join("cycle")).unwrap();
        let (sender, _) = mpsc::channel();
        let paths = discover_audio_paths(root.path(), &AtomicBool::new(false), &sender).unwrap();
        assert_eq!(
            paths,
            vec![root.path().join("kick.wav"), child.join("snare.WAV")]
        );
        assert!(discover_audio_paths(root.path(), &AtomicBool::new(true), &sender).is_err());
        assert!(
            discover_audio_paths(
                &root.path().join("kick.wav"),
                &AtomicBool::new(false),
                &sender
            )
            .is_err()
        );
    }

    #[test]
    fn unavailable_audio_fails_analysis_instead_of_publishing_partial_results() {
        let root = tempfile::tempdir().unwrap();
        let (sender, _) = mpsc::channel();
        assert!(
            analyze_audio_paths(
                &[root.path().join("missing.wav")],
                root.path(),
                &AtomicBool::new(false),
                &sender
            )
            .is_err()
        );
        std::fs::write(
            root.path().join("unsupported.wav"),
            b"not an audio container",
        )
        .unwrap();
        let samples = analyze_audio_paths(
            &[root.path().join("unsupported.wav")],
            root.path(),
            &AtomicBool::new(false),
            &sender,
        )
        .unwrap();
        assert!(
            samples.is_empty(),
            "unsupported content is a classification omission, not an inaccessible source"
        );
    }

    #[test]
    fn over_budget_sample_rate_is_rejected_before_analysis_allocation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("unsupported-rate.wav");
        let frames = 40_000_u32;
        let rate = 400_000_u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(frames * 2).to_le_bytes());
        for index in 0..frames {
            let value: i16 = if index % 100 < 50 { 12_000 } else { -12_000 };
            wav.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        let decoded =
            Decoder::try_from(BufReader::new(std::fs::File::open(&path).unwrap())).unwrap();
        assert_eq!(
            decoded.sample_rate().get(),
            rate,
            "fixture must reach the header-budget check"
        );
        assert!(
            process_single_audio_file(&path, "unsupported-rate.wav".into(), root.path())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_is_audio_file() {
        assert!(is_audio_file(Path::new("kick.wav")));
        assert!(is_audio_file(Path::new("snare.flac")));
        assert!(is_audio_file(Path::new("hat.mp3")));
        assert!(is_audio_file(Path::new("synth.aiff")));
        assert!(!is_audio_file(Path::new("notes.txt")));
        assert!(!is_audio_file(Path::new(".hidden.wav")));
        assert!(!is_audio_file(Path::new("._mac_meta.wav")));
    }

    #[test]
    fn rejects_protected_audio_inputs() {
        assert!(
            process_single_audio_file(Path::new("/dev/zero"), "zero.wav".into(), Path::new("/dev"))
                .is_err()
        );
    }
}
