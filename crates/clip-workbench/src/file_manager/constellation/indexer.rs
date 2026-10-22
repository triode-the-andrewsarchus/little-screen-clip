//! Background worker for scanning and indexing audio sample folders into SQLite.

use super::classifier::classify_sample;
use super::db::{ConstellationDb, ConstellationSample};
use rodio::{Decoder, Source};
use std::collections::VecDeque;
use std::fs::File;
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

pub fn run_indexing(
    folder: PathBuf,
    cancelled: Arc<AtomicBool>,
    sender: mpsc::Sender<IndexProgress>,
) {
    let is_cancelled = || cancelled.load(Ordering::Relaxed);

    // 1. Recursive directory walk to discover audio files
    let mut audio_paths = Vec::new();
    let mut dir_queue = VecDeque::new();
    dir_queue.push_back(folder.clone());

    while let Some(current_dir) = dir_queue.pop_front() {
        if is_cancelled() {
            return;
        }

        if let Ok(entries) = std::fs::read_dir(&current_dir) {
            for entry_res in entries {
                if is_cancelled() {
                    return;
                }
                let Ok(entry) = entry_res else { continue };
                let p = entry.path();
                let filename = entry.file_name().to_string_lossy().to_string();

                if filename.starts_with('.') || filename.starts_with("._") || filename == "__MACOSX"
                {
                    continue;
                }

                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        dir_queue.push_back(p);
                    } else if ft.is_file() && is_audio_file(&p) {
                        audio_paths.push(p);
                        if audio_paths.len() % 50 == 0 {
                            let _ = sender.send(IndexProgress::Scanning {
                                files_found: audio_paths.len(),
                            });
                        }
                    }
                }
            }
        }
    }

    let total = audio_paths.len();
    if total == 0 {
        let _ = sender.send(IndexProgress::Complete {
            samples: Vec::new(),
        });
        return;
    }

    // 2. Parallel processing of audio files using a scoped worker pool
    let num_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 16)
        .min(total);

    let next_file_idx = AtomicUsize::new(0);
    let processed_counter = AtomicUsize::new(0);

    let audio_paths_ref = &audio_paths;
    let folder_ref = &folder;
    let cancelled_ref = &cancelled;
    let next_file_idx_ref = &next_file_idx;
    let processed_counter_ref = &processed_counter;

    let mut worker_results: Vec<Vec<ConstellationSample>> = Vec::with_capacity(num_workers);

    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(num_workers);
        for _ in 0..num_workers {
            let thread_sender = sender.clone();
            let handle = s.spawn(move || {
                let mut local_samples = Vec::with_capacity(total / num_workers + 16);
                while !cancelled_ref.load(Ordering::Relaxed) {
                    let idx = next_file_idx_ref.fetch_add(1, Ordering::Relaxed);
                    if idx >= total {
                        break;
                    }

                    let path = &audio_paths_ref[idx];
                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "sample.wav".to_string());

                    let count = processed_counter_ref.fetch_add(1, Ordering::Relaxed) + 1;
                    if count % 10 == 0 || count == total {
                        let _ = thread_sender.send(IndexProgress::Indexing {
                            processed: count,
                            total,
                            current_file: filename.clone(),
                        });
                    }

                    if let Some(sample) = process_single_audio_file(path, filename, folder_ref) {
                        local_samples.push(sample);
                    }
                }
                local_samples
            });
            handles.push(handle);
        }

        for h in handles {
            if let Ok(res) = h.join() {
                worker_results.push(res);
            }
        }
    });

    if is_cancelled() {
        return;
    }

    let mut indexed_samples = Vec::with_capacity(total);
    for mut res in worker_results {
        indexed_samples.append(&mut res);
    }

    // 3. Save to database
    if let Ok(db) = ConstellationDb::new() {
        if let Err(e) = db.save_samples(&folder, &indexed_samples) {
            let _ = sender.send(IndexProgress::Error(format!(
                "Failed to save indexed samples to database: {e}"
            )));
            return;
        }
    }

    let _ = sender.send(IndexProgress::Complete {
        samples: indexed_samples,
    });
}

fn process_single_audio_file(
    path: &Path,
    filename: String,
    root_folder: &Path,
) -> Option<ConstellationSample> {
    let file = File::open(path).ok()?;
    let meta = file.metadata().ok();
    let file_size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut decoder = Decoder::try_from(BufReader::new(file)).ok()?;
    let sample_rate = decoder.sample_rate().get() as f32;
    let channels = decoder.channels().get() as usize;
    if sample_rate <= 0.0 || channels == 0 {
        return None;
    }

    // Decode first 1.5 seconds into mono f32 samples
    let max_samples_per_ch = (sample_rate * 1.5) as usize;
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
        return None;
    }

    let classification = classify_sample(path, &mono_samples, sample_rate)?;
    let parent_folder = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| root_folder.to_path_buf());

    Some(ConstellationSample {
        file_path: path.to_path_buf(),
        folder_path: parent_folder,
        filename,
        category: classification.category,
        confidence: classification.confidence,
        duration_seconds: classification.duration,
        pitch_hz: classification.pitch_hz,
        pitch_clarity: classification.pitch_clarity,
        brightness: classification.brightness,
        x: classification.x,
        y: classification.y,
        file_size,
        mtime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_indexing_real_sample_folder_if_available() {
        let sample_folder = PathBuf::from("/mnt/rust/music/Music/Samples/BPB Cluster Pack/Samples");
        if !sample_folder.exists() {
            return;
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();

        run_indexing(sample_folder.clone(), cancelled, sender);

        let mut final_samples = Vec::new();
        while let Ok(msg) = receiver.recv() {
            match msg {
                IndexProgress::Complete { samples } => {
                    final_samples = samples;
                    break;
                }
                IndexProgress::Error(e) => {
                    panic!("Indexing error: {e}");
                }
                _ => {}
            }
        }

        assert!(!final_samples.is_empty(), "Should have indexed samples");
        println!(
            "Successfully indexed {} samples from {:?}",
            final_samples.len(),
            sample_folder
        );
    }
}
