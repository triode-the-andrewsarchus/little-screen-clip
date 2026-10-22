//! Sample pack creation shelf, slot layering, zero-crossing trimming, and bank persistence.
//! Saves banks to `little-screen-clip-sample-packs` via symlinks and metadata JSON.

use super::generated_model::SampleCategory;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PACKS_DIR_NAME: &str = "little-screen-clip-sample-packs";
pub const NUM_SLOTS: usize = 10;
pub const SLOT_KEY_CHARS: [char; NUM_SLOTS] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];

/// A single sample layer assigned to a pack slot.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SlotSample {
    pub file_path: PathBuf,
    pub filename: String,
    pub category: Option<SampleCategory>,
    pub duration: f32,
    pub start_sec: f32,
    pub end_sec: f32,
    pub volume: f32,
    #[serde(skip)]
    pub pcm_cache: Option<Arc<Vec<f32>>>,
    #[serde(skip)]
    pub sample_rate: u32,
}

impl SlotSample {
    pub fn new(
        file_path: PathBuf,
        filename: String,
        category: Option<SampleCategory>,
        duration: f32,
    ) -> Self {
        let dur = duration.max(0.01);
        let mut sample = Self {
            file_path,
            filename,
            category,
            duration: dur,
            start_sec: 0.0,
            end_sec: dur,
            volume: 1.0,
            pcm_cache: None,
            sample_rate: 0,
        };
        sample.ensure_pcm_loaded();
        sample
    }

    /// Loads the first few seconds of decoded mono PCM into memory for zero-crossing snapping.
    pub fn ensure_pcm_loaded(&mut self) {
        if self.pcm_cache.is_some() && self.sample_rate > 0 {
            return;
        }
        if let Ok(file) = File::open(&self.file_path) {
            if let Ok(mut decoder) = rodio::Decoder::try_from(BufReader::new(file)) {
                use rodio::Source;
                let sr = decoder.sample_rate().get();
                let channels = decoder.channels().get() as usize;
                self.sample_rate = sr;
                // Cache up to 5 seconds of mono PCM
                let max_frames = (sr * 5) as usize;
                let mut mono = Vec::with_capacity(max_frames);
                let mut ch_idx = 0;
                let mut frame_sum = 0.0f32;
                for s in decoder.by_ref().take(max_frames * channels) {
                    frame_sum += s;
                    ch_idx += 1;
                    if ch_idx >= channels {
                        mono.push(frame_sum / channels as f32);
                        frame_sum = 0.0;
                        ch_idx = 0;
                    }
                }
                self.pcm_cache = Some(Arc::new(mono));
            }
        }
    }

    /// Snaps a given time offset to the nearest zero crossing in the audio sample.
    pub fn snap_time(&mut self, time_sec: f32) -> f32 {
        self.ensure_pcm_loaded();
        if let Some(ref pcm) = self.pcm_cache {
            if self.sample_rate > 0 && !pcm.is_empty() {
                return little_clip_core::audio::snap_samples_to_zero_crossing(
                    pcm,
                    None,
                    self.sample_rate,
                    time_sec,
                    500, // 500 frames ≈ 11ms at 44.1kHz
                );
            }
        }
        time_sec
    }
}

/// One of the 10 slots (Keys 1..9, 0) in the sample pack bank.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PackSlot {
    pub key_char: char,
    pub name: String,
    pub samples: Vec<SlotSample>,
}

impl PackSlot {
    pub fn new(key_char: char) -> Self {
        Self {
            key_char,
            name: format!("Slot {}", key_char),
            samples: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn display_title(&self) -> String {
        if self.samples.is_empty() {
            format!("Slot {} (Empty)", self.key_char)
        } else if self.samples.len() == 1 {
            self.samples[0].filename.clone()
        } else {
            format!(
                "{} ({} layers)",
                self.samples[0].filename,
                self.samples.len()
            )
        }
    }
}

/// An entire 10-slot sample pack bank.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SamplePackBank {
    pub name: String,
    pub created_at: u64,
    pub slots: Vec<PackSlot>,
}

impl Default for SamplePackBank {
    fn default() -> Self {
        Self::new("Untitled Pack")
    }
}

impl SamplePackBank {
    pub fn new(name: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let slots = SLOT_KEY_CHARS.iter().map(|&k| PackSlot::new(k)).collect();
        Self {
            name: name.to_string(),
            created_at: now,
            slots,
        }
    }

    pub fn total_samples(&self) -> usize {
        self.slots.iter().map(|s| s.samples.len()).sum()
    }

    pub fn filled_slots_count(&self) -> usize {
        self.slots.iter().filter(|s| !s.is_empty()).count()
    }
}

/// Metadata summary of a saved pack on disk.
#[derive(Clone, Debug)]
pub struct SamplePackSummary {
    pub name: String,
    pub folder_path: PathBuf,
    pub slot_count: usize,
    pub sample_count: usize,
    pub modified: SystemTime,
}

/// Resolves the root packs directory `<root>/little-screen-clip-sample-packs`.
pub fn get_sample_packs_root(constellation_root: &Path) -> PathBuf {
    constellation_root.join(PACKS_DIR_NAME)
}

/// Saves a sample pack bank to `<constellation_root>/little-screen-clip-sample-packs/<bank_name>/`.
/// Generates symlinks for every assigned sample and writes `bank.json`.
pub fn save_sample_pack(
    constellation_root: &Path,
    bank: &SamplePackBank,
) -> Result<PathBuf, String> {
    let sanitized_name = bank
        .name
        .trim()
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    let folder_name = if sanitized_name.is_empty() {
        "Default_Pack".to_string()
    } else {
        sanitized_name
    };

    let packs_root = get_sample_packs_root(constellation_root);
    let pack_dir = packs_root.join(&folder_name);
    std::fs::create_dir_all(&pack_dir)
        .map_err(|e| format!("Failed to create sample pack directory: {e}"))?;

    // Clean existing symlinks in pack_dir so stale files don't linger
    if let Ok(entries) = std::fs::read_dir(&pack_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) != Some("bank.json") {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    // Create numbered symlinks for each assigned sample
    for (slot_idx, slot) in bank.slots.iter().enumerate() {
        for (layer_idx, sample) in slot.samples.iter().enumerate() {
            let ext = sample
                .file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("wav");
            let stem = sample
                .file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("sample");

            let symlink_name = if slot.samples.len() == 1 {
                format!("{:02}_{}_{}.{}", slot_idx + 1, slot.key_char, stem, ext)
            } else {
                format!(
                    "{:02}_{}_layer{}_{}.{}",
                    slot_idx + 1,
                    slot.key_char,
                    layer_idx + 1,
                    stem,
                    ext
                )
            };

            let symlink_path = pack_dir.join(&symlink_name);
            let _ = std::fs::remove_file(&symlink_path);

            #[cfg(unix)]
            {
                let _ = std::os::unix::fs::symlink(&sample.file_path, &symlink_path);
            }
            #[cfg(windows)]
            {
                let _ = std::os::windows::fs::symlink_file(&sample.file_path, &symlink_path);
            }
        }
    }

    // Save bank.json
    let json_path = pack_dir.join("bank.json");
    let json_str = serde_json::to_string_pretty(bank)
        .map_err(|e| format!("Failed to serialize pack bank: {e}"))?;
    std::fs::write(&json_path, json_str).map_err(|e| format!("Failed to write bank.json: {e}"))?;

    Ok(pack_dir)
}

/// Loads a sample pack bank from its directory containing `bank.json`.
pub fn load_sample_pack(pack_dir: &Path) -> Result<SamplePackBank, String> {
    let json_path = pack_dir.join("bank.json");
    if !json_path.is_file() {
        return Err(format!("No bank.json found in {}", pack_dir.display()));
    }

    let json_str = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("Failed to read bank.json: {e}"))?;
    let mut bank: SamplePackBank =
        serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse bank.json: {e}"))?;

    // Ensure all 10 slots are present
    while bank.slots.len() < NUM_SLOTS {
        let key = SLOT_KEY_CHARS[bank.slots.len()];
        bank.slots.push(PackSlot::new(key));
    }

    // Re-link and preload PCM for active samples
    for slot in &mut bank.slots {
        for sample in &mut slot.samples {
            sample.ensure_pcm_loaded();
        }
    }

    Ok(bank)
}

/// Lists all saved sample packs in `<constellation_root>/little-screen-clip-sample-packs`.
pub fn list_saved_packs(constellation_root: &Path) -> Vec<SamplePackSummary> {
    let packs_root = get_sample_packs_root(constellation_root);
    let mut packs = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&packs_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Pack".to_string());
                let meta = entry.metadata().ok();
                let modified = meta.and_then(|m| m.modified().ok()).unwrap_or(UNIX_EPOCH);

                if let Ok(bank) = load_sample_pack(&path) {
                    let bank_name = bank.name.clone();
                    let slot_count = bank.filled_slots_count();
                    let sample_count = bank.total_samples();
                    packs.push(SamplePackSummary {
                        name: bank_name,
                        folder_path: path,
                        slot_count,
                        sample_count,
                        modified,
                    });
                } else {
                    // Check for loose audio files or symlinks
                    let mut file_count = 0;
                    if let Ok(sub) = std::fs::read_dir(&path) {
                        for sub_entry in sub.flatten() {
                            if sub_entry.path().is_file() {
                                file_count += 1;
                            }
                        }
                    }
                    if file_count > 0 {
                        packs.push(SamplePackSummary {
                            name,
                            folder_path: path,
                            slot_count: file_count.min(10),
                            sample_count: file_count,
                            modified,
                        });
                    }
                }
            }
        }
    }

    packs.sort_by(|a, b| b.modified.cmp(&a.modified));
    packs
}

/// Deletes a saved sample pack directory.
pub fn delete_sample_pack(pack_dir: &Path) -> Result<(), String> {
    if pack_dir.is_dir() {
        std::fs::remove_dir_all(pack_dir)
            .map_err(|e| format!("Failed to delete pack directory: {e}"))?;
        Ok(())
    } else {
        Err("Directory does not exist".into())
    }
}

/// Plays a single `SlotSample` using `new_shared_audio_player` with trimmed bounds and volume.
pub fn play_slot_sample(sample: &SlotSample) -> Option<rodio::Player> {
    let player = crate::ui_ext::new_shared_audio_player()?;
    let file = File::open(&sample.file_path).ok()?;
    let decoder = rodio::Decoder::try_from(BufReader::new(file)).ok()?;
    use rodio::Source;

    let start = Duration::from_secs_f32(sample.start_sec.max(0.0));
    let dur = Duration::from_secs_f32((sample.end_sec - sample.start_sec).max(0.005));
    let bounded = decoder
        .skip_duration(start)
        .take_duration(dur)
        .amplify(sample.volume);

    player.append(bounded);
    player.play();
    Some(player)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_pack_bank_initialization() {
        let bank = SamplePackBank::new("Test Kit");
        assert_eq!(bank.name, "Test Kit");
        assert_eq!(bank.slots.len(), 10);
        assert_eq!(bank.slots[0].key_char, '1');
        assert_eq!(bank.slots[8].key_char, '9');
        assert_eq!(bank.slots[9].key_char, '0');
        assert_eq!(bank.total_samples(), 0);
        assert_eq!(bank.filled_slots_count(), 0);
    }

    #[test]
    fn test_sample_pack_save_and_load_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!("pack_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);

        // Create dummy sample audio file
        let sample_file = temp_dir.join("test_kick.wav");
        std::fs::write(&sample_file, b"RIFF....WAVEfmt ....data....").unwrap();

        let mut bank = SamplePackBank::new("Boom Bap Pack");
        bank.slots[0].samples.push(SlotSample {
            file_path: sample_file.clone(),
            filename: "test_kick.wav".to_string(),
            category: Some(SampleCategory::Kick),
            duration: 0.5,
            start_sec: 0.05,
            end_sec: 0.45,
            volume: 1.2,
            pcm_cache: None,
            sample_rate: 44100,
        });

        let pack_path = save_sample_pack(&temp_dir, &bank).expect("Failed to save pack");
        assert!(pack_path.exists());
        assert!(pack_path.join("bank.json").is_file());

        let loaded = load_sample_pack(&pack_path).expect("Failed to load pack");
        assert_eq!(loaded.name, "Boom Bap Pack");
        assert_eq!(loaded.slots[0].samples.len(), 1);
        assert_eq!(loaded.slots[0].samples[0].filename, "test_kick.wav");
        assert!((loaded.slots[0].samples[0].start_sec - 0.05).abs() < 1e-4);
        assert!((loaded.slots[0].samples[0].end_sec - 0.45).abs() < 1e-4);
        assert!((loaded.slots[0].samples[0].volume - 1.2).abs() < 1e-4);

        let list = list_saved_packs(&temp_dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Boom Bap Pack");
        assert_eq!(list[0].sample_count, 1);

        let _ = delete_sample_pack(&pack_path);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
