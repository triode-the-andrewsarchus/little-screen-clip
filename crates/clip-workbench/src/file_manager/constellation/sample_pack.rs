//! Layered sample references, bounded PCM preparation, and atomic pack persistence.
//!
//! Banks contain sixteen pads with up to eight layers each. Saved packs retain original
//! source paths through symlinks and `bank.json`; trim/volume edits are metadata, not
//! rendered audio. Existing packs remain intact: repeated saves create numbered
//! versions, and the new directory appears only after staging succeeds.

use super::generated_model::SampleCategory;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PACKS_DIR_NAME: &str = "little-screen-clip-sample-packs";
pub const NUM_SLOTS: usize = 16;
pub const SLOT_KEY_CHARS: [char; NUM_SLOTS] = [
    '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', ' ', ' ', ' ', ' ', ' ', ' ',
];

/// Prepared interleaved PCM, mono snapping data, and frequency-colored min/max waveform buckets.
///
/// Slot layers share this allocation through [`Arc<T>`]. Playback owns a frame-aligned
/// view of its PCM and never copies the trimmed payload on the UI thread.
#[derive(Clone)]
pub struct CachedAudio {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Arc<Vec<f32>>,
    pub mono_samples: Arc<Vec<f32>>,
    pub waveform_peaks: Vec<(f32, f32)>,
    pub waveform_colors: Vec<[u8; 3]>,
}

impl std::fmt::Debug for CachedAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAudio")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("samples_len", &self.samples.len())
            .field("peaks_len", &self.waveform_peaks.len())
            .finish()
    }
}

impl CachedAudio {
    /// Decodes one ordinary source, rejecting a source replaced during the read.
    fn load(path: &Path) -> Option<Self> {
        use rodio::Source;
        use std::os::fd::AsRawFd;
        let file = little_clip_core::kernel_interface::open_content(path, true).ok()?;
        let source = crate::file_source::FileSource::capture(Path::new(&format!(
            "/proc/self/fd/{}",
            file.as_raw_fd()
        )))
        .ok()?;
        let decoder = rodio::Decoder::try_from(BufReader::new(file)).ok()?;
        let sample_rate = decoder.sample_rate().get();
        let channels = decoder.channels().get();
        if sample_rate > super::MAX_SAMPLE_RATE || channels > super::MAX_SAMPLE_CHANNELS {
            return None;
        }
        let width = channels as usize;
        let frames = (sample_rate as usize * 15).min(2_000_000 / width);
        let mut samples: Vec<_> = decoder.take(frames * width).collect();
        samples.truncate(samples.len() / width * width);
        if samples.is_empty() || samples.iter().any(|sample| !sample.is_finite()) {
            return None;
        }
        let mono: Vec<f32> = samples
            .chunks_exact(width)
            .map(|frame| frame.iter().sum::<f32>() / width as f32)
            .collect();
        let buckets = mono.len().min(512);
        let mut color = little_clip_core::audio::WaveformColor::new(sample_rate as f32);
        let mut waveform_colors = Vec::with_capacity(buckets);
        let waveform_peaks = (0..buckets)
            .map(|bucket| {
                let start = bucket * mono.len() / buckets;
                let end = ((bucket + 1) * mono.len() / buckets)
                    .max(start + 1)
                    .min(mono.len());
                color.push_samples(&mono[start..end]);
                waveform_colors.push(color.take_color());
                mono[start..end]
                    .iter()
                    .fold((0.0_f32, 0.0_f32), |(low, high), &value| {
                        (low.min(value), high.max(value))
                    })
            })
            .collect();
        source.check().ok()?;
        Some(Self {
            channels,
            sample_rate,
            samples: Arc::new(samples),
            mono_samples: Arc::new(mono),
            waveform_peaks,
            waveform_colors,
        })
    }
}

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
    /// Stereo balance: -1 is left, 0 preserves both channels, 1 is right.
    #[serde(default)]
    pub pan: f32,
    /// Linear fade at the trimmed start, in milliseconds (0–500).
    #[serde(default)]
    pub attack_ms: f32,
    /// Linear fade ending at the trimmed end, in milliseconds (0–500).
    #[serde(default)]
    pub release_ms: f32,
    #[serde(skip)]
    pub cached_audio: Option<Arc<CachedAudio>>,
}

// Preparation state is not a document edit. Native path comparison also retains
// non-UTF-8 identity, unlike using serialized JSON as an undo/dirty fingerprint.
impl PartialEq for SlotSample {
    fn eq(&self, other: &Self) -> bool {
        self.file_path == other.file_path
            && self.filename == other.filename
            && self.category == other.category
            && self.duration == other.duration
            && self.start_sec == other.start_sec
            && self.end_sec == other.end_sec
            && self.volume == other.volume
            && self.pan == other.pan
            && self.attack_ms == other.attack_ms
            && self.release_ms == other.release_ms
    }
}

impl SlotSample {
    pub fn new(
        file_path: PathBuf,
        filename: String,
        category: Option<SampleCategory>,
        duration: f32,
    ) -> Self {
        let dur = duration.max(0.000_001);
        Self {
            file_path,
            filename,
            category,
            duration: dur,
            start_sec: 0.0,
            end_sec: dur,
            volume: 1.0,
            pan: 0.0,
            attack_ms: 0.0,
            release_ms: 0.0,
            cached_audio: None,
        }
    }

    /// Loads bounded PCM and peaks on a background worker if they are not cached.
    ///
    /// Reads at most 15 seconds and two million interleaved values. Unsupported,
    /// replaced, or undecodable sources leave the cache empty. This performs
    /// filesystem I/O and decoding; callers must keep it off the UI thread.
    pub fn ensure_audio_loaded(&mut self) {
        if self.cached_audio.is_none() {
            if let Some(audio) = CachedAudio::load(&self.file_path) {
                self.set_cached_audio(Arc::new(audio));
            }
        }
    }

    /// Adopts prepared PCM while preserving user trim bounds that still fit.
    pub(super) fn set_cached_audio(&mut self, audio: Arc<CachedAudio>) {
        let duration =
            audio.samples.len() as f32 / audio.channels as f32 / audio.sample_rate as f32;
        if self.end_sec == self.duration {
            self.end_sec = duration;
        }
        self.duration = duration;
        self.end_sec = self.end_sec.min(duration);
        self.start_sec = self
            .start_sec
            .min((self.end_sec - 1.0 / audio.sample_rate as f32).max(0.0));
        self.cached_audio = Some(audio);
    }

    fn validate(&self) -> Result<(), String> {
        if !self.duration.is_finite()
            || self.duration <= 0.0
            || !self.start_sec.is_finite()
            || !self.end_sec.is_finite()
            || self.start_sec < 0.0
            || self.end_sec <= self.start_sec
            || self.end_sec > self.duration
            || !self.volume.is_finite()
            || !(0.0..=2.0).contains(&self.volume)
            || !(-1.0..=1.0).contains(&self.pan)
            || !(0.0..=500.0).contains(&self.attack_ms)
            || !(0.0..=500.0).contains(&self.release_ms)
        {
            return Err("Pack contains invalid trim bounds or volume".into());
        }
        Ok(())
    }

    /// Returns the nearest mono zero crossing within 500 frames of `time_sec`.
    ///
    /// Uses core's shared search and returns the input unchanged until PCM is ready.
    /// This method does no I/O; callers clamp the result to their active trim range.
    pub fn snap_time(&self, time_sec: f32) -> f32 {
        if let Some(ref cached) = self.cached_audio {
            if cached.sample_rate > 0 && !cached.mono_samples.is_empty() {
                return little_clip_core::audio::snap_samples_to_zero_crossing(
                    &cached.mono_samples,
                    None,
                    cached.sample_rate,
                    time_sec,
                    500, // 500 frames ≈ 11ms at 44.1kHz
                );
            }
        }
        time_sec
    }
}

/// One of sixteen pads, ordered left to right from the bottom row in the sample pack bank.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PackSlot {
    /// Legacy bank metadata; active keyboard bindings come from local configuration.
    pub key_char: char,
    pub name: String,
    pub samples: Vec<SlotSample>,
    /// Zero disables choking; pads in the same group (1–8) cut each other off.
    #[serde(default)]
    pub choke_group: u8,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
}

impl PackSlot {
    pub fn new(key_char: char) -> Self {
        Self {
            key_char,
            name: format!("Slot {}", key_char),
            samples: Vec::new(),
            choke_group: 0,
            mute: false,
            solo: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn display_title(&self) -> String {
        if self.samples.is_empty() {
            "Empty pad".into()
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

/// A sixteen-pad bank. Legacy ten-slot banks keep their original shortcut positions.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SamplePackBank {
    pub name: String,
    pub created_at: u64,
    pub slots: Vec<PackSlot>,
    /// Forces MIDI hits to full velocity; keyboard and mouse hits are always full.
    #[serde(default)]
    pub override_velocity: bool,
    /// Bypasses dynamics processing on the pad output bus.
    #[serde(default)]
    pub compressor_bypass: bool,
    /// Stereo bus threshold in dBFS, after the fixed 2 dB input headroom.
    #[serde(default = "default_compressor_threshold")]
    pub compressor_threshold_db: f32,
    /// Compression ratio; one disables compression but retains the peak ceiling.
    #[serde(default = "default_compressor_ratio")]
    pub compressor_ratio: f32,
}

fn default_compressor_threshold() -> f32 {
    -1.411_621_5
}
fn default_compressor_ratio() -> f32 {
    2.0
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
            override_velocity: false,
            compressor_bypass: false,
            compressor_threshold_db: default_compressor_threshold(),
            compressor_ratio: default_compressor_ratio(),
        }
    }

    pub(super) fn validate(&mut self, pack_dir: &Path) -> Result<(), String> {
        if !(-48.0..=0.0).contains(&self.compressor_threshold_db)
            || !(1.0..=12.0).contains(&self.compressor_ratio)
        {
            return Err("Compressor threshold must be -48 to 0 dBFS and ratio 1 to 12".into());
        }
        if self.slots.len() > NUM_SLOTS {
            return Err("Pack has more than sixteen pads".into());
        }
        while self.slots.len() < NUM_SLOTS {
            self.slots
                .push(PackSlot::new(SLOT_KEY_CHARS[self.slots.len()]));
        }
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            slot.key_char = SLOT_KEY_CHARS[idx];
            if slot.choke_group > 8 {
                return Err("Choke groups must be between zero and eight".into());
            }
            if slot.samples.len() > 8 {
                return Err("A slot supports at most eight layers".into());
            }
            for sample in &mut slot.samples {
                sample.validate()?;
                if sample.file_path.is_relative() {
                    sample.file_path = pack_dir.join(&sample.file_path);
                }
            }
        }
        Ok(())
    }

    /// Stages metadata and links while retaining every validated source handle.
    fn stage_references(&self, staged: &Path) -> Result<Vec<File>, String> {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let mut saved = self.clone();
        saved.validate(staged)?;
        let mut sources = Vec::new();
        for (slot_idx, slot) in saved.slots.iter_mut().enumerate() {
            for (layer_idx, sample) in slot.samples.iter_mut().enumerate() {
                let source =
                    little_clip_core::kernel_interface::open_content(&sample.file_path, true)
                        .map_err(|e| format!("Cannot save {}: {e}", sample.filename))?;
                sample.file_path =
                    std::fs::read_link(format!("/proc/self/fd/{}", source.as_raw_fd()))
                        .map_err(|e| e.to_string())?;
                let filename = sample
                    .file_path
                    .file_name()
                    .ok_or("Missing sample filename")?
                    .to_string_lossy();
                let name = format!("{:02}_layer{}_{}", slot_idx + 1, layer_idx + 1, filename);
                std::os::unix::fs::symlink(&sample.file_path, staged.join(name))
                    .map_err(|e| e.to_string())?;
                sources.push(source);
            }
        }
        let mut json = File::create(staged.join("bank.json")).map_err(|e| e.to_string())?;
        json.write_all(
            serde_json::to_string_pretty(&saved)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        json.sync_all().map_err(|e| e.to_string())?;
        Ok(sources)
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

/// Stages a complete reference pack and atomically installs it under the sample root.
///
/// Keeps validated output-directory and source handles until installation. Existing
/// names are never overwritten; numbered suffixes retain previous versions. This
/// performs filesystem I/O and must run off the UI thread with an unbounded job
/// lifetime so a timeout cannot hide a completed mutation.
///
/// # Errors
/// Returns an error for invalid banks/names, protected or missing sources/output
/// directories, serialization failures, or failed staging/installation.
pub fn save_sample_pack(
    constellation_root: &Path,
    bank: &SamplePackBank,
) -> Result<PathBuf, String> {
    install_pack(constellation_root, bank, |staged| {
        bank.stage_references(staged)
    })
}

/// Shares pinned-directory staging and no-overwrite publication across pack formats.
pub(super) fn install_pack(
    constellation_root: &Path,
    bank: &SamplePackBank,
    stage: impl FnOnce(&Path) -> Result<Vec<File>, String>,
) -> Result<PathBuf, String> {
    use std::os::fd::AsRawFd;
    let sanitized_name = bank
        .name
        .trim()
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    if matches!(sanitized_name.as_str(), "." | "..") {
        return Err("Choose a pack name other than '.' or '..'".into());
    }
    let folder_name = if sanitized_name.is_empty() {
        "Default_Pack"
    } else {
        &sanitized_name
    };
    let packs_root = get_sample_packs_root(constellation_root);
    let directory = little_clip_core::kernel_interface::create_content_directory(&packs_root)
        .map_err(|e| e.to_string())?;
    let parent = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let staged = tempfile::Builder::new()
        .prefix(".pack-")
        .tempdir_in(&parent)
        .map_err(|e| e.to_string())?;
    let _sources = stage(staged.path())?;
    // Publish a complete pack atomically. Repeated saves retain previous versions.
    for suffix in 0..10_000 {
        let name = if suffix == 0 {
            folder_name.to_string()
        } else {
            format!("{folder_name} ({suffix})")
        };
        match super::super::filesystem::rename_noreplace(staged.path(), &parent.join(&name)) {
            Ok(()) => return Ok(packs_root.join(name)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot install sample pack: {error}")),
        }
    }
    Err("Too many saved versions; choose another pack name".into())
}

/// Loads and validates at most 1 MiB of bank metadata without decoding audio.
///
/// Pads missing slots, restores canonical shortcut keys, and resolves relative
/// source references against `pack_dir`. PCM is prepared separately on workers.
///
/// # Errors
/// Returns an error for unavailable/protected metadata, malformed JSON, excessive
/// slot/layer counts, or non-finite/out-of-range trim, duration, and volume values.
pub fn load_sample_pack(pack_dir: &Path) -> Result<SamplePackBank, String> {
    use std::io::Read;
    let file = little_clip_core::kernel_interface::open_content(&pack_dir.join("bank.json"), true)
        .map_err(|e| e.to_string())?;
    let mut json = String::new();
    file.take(1_048_577)
        .read_to_string(&mut json)
        .map_err(|e| e.to_string())?;
    if json.len() > 1_048_576 {
        return Err("Pack metadata exceeds 1 MiB".into());
    }
    let mut bank: SamplePackBank = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    bank.validate(pack_dir)?;
    Ok(bank)
}

/// Lists all saved sample packs in `<constellation_root>/little-screen-clip-sample-packs`.
pub fn list_saved_packs(constellation_root: &Path) -> Vec<SamplePackSummary> {
    let packs_root = get_sample_packs_root(constellation_root);
    let mut packs = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&packs_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && !entry.file_name().to_string_lossy().starts_with('.')
            {
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
                }
            }
        }
    }

    packs.sort_by(|a, b| b.modified.cmp(&a.modified));
    packs
}

/// Deletes an ordinary, validated pack directory inside [`PACKS_DIR_NAME`].
///
/// # Errors
/// Returns an error for linked directories, invalid metadata, protected payload
/// trees, or filesystem failures. Callers must preserve accepted completions.
pub fn delete_sample_pack(pack_dir: &Path) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let parent_path = pack_dir
        .parent()
        .filter(|parent| {
            parent
                .file_name()
                .is_some_and(|name| name == PACKS_DIR_NAME)
        })
        .ok_or("Not a sample pack directory")?;
    let directory = little_clip_core::kernel_interface::pin_content_directory(parent_path)
        .map_err(|e| e.to_string())?;
    let parent = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let target = parent.join(pack_dir.file_name().ok_or("Missing pack name")?);
    if std::fs::symlink_metadata(&target)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("Refusing to delete a linked pack directory".into());
    }
    load_sample_pack(&target)?;
    super::super::system_route::tree(&target, &std::sync::atomic::AtomicBool::new(false))?;
    std::fs::remove_dir_all(&target).map_err(|e| format!("Failed to delete pack directory: {e}"))
}

/// A frame-aligned view over shared PCM. Triggering a slot never copies the audio buffer.
pub(super) struct SharedPcm {
    audio: Arc<CachedAudio>,
    position: usize,
    end: usize,
    length: usize,
}
impl SharedPcm {
    pub(super) fn trimmed(sample: &SlotSample) -> Option<Self> {
        let audio = sample.cached_audio.as_ref()?.clone();
        if audio.channels == 0
            || audio.sample_rate == 0
            || !sample.start_sec.is_finite()
            || !sample.end_sec.is_finite()
            || sample.start_sec < 0.0
            || sample.end_sec <= sample.start_sec
        {
            return None;
        }
        let channels = audio.channels as usize;
        let frames = audio.samples.len() / channels;
        let start = ((sample.start_sec as f64 * audio.sample_rate as f64).round() as usize)
            .min(frames)
            * channels;
        let end = ((sample.end_sec as f64 * audio.sample_rate as f64).round() as usize).min(frames)
            * channels;
        (end > start).then_some(Self {
            audio,
            position: start,
            end,
            length: end - start,
        })
    }
}
impl Iterator for SharedPcm {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.position >= self.end {
            return None;
        }
        let sample = self.audio.samples[self.position];
        self.position += 1;
        Some(sample)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.end - self.position;
        (remaining, Some(remaining))
    }
}
impl rodio::Source for SharedPcm {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.end - self.position)
    }
    fn channels(&self) -> rodio::ChannelCount {
        std::num::NonZeroU16::new(self.audio.channels).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        std::num::NonZeroU32::new(self.audio.sample_rate).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.length as f64 / self.audio.channels as f64 / self.audio.sample_rate as f64,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_waveform_colors_cover_every_bucket_including_tiny_hits() {
        let root = tempfile::tempdir().unwrap();
        for frames in [16, 2048] {
            let path = root.path().join(format!("{frames}.wav"));
            let mut writer = hound::WavWriter::create(
                &path,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 48000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for frame in 0..frames {
                writer
                    .write_sample(((frame as f32 * 0.13).sin() * 16000.0) as i16)
                    .unwrap();
            }
            writer.finalize().unwrap();
            let audio = CachedAudio::load(&path).unwrap();
            assert_eq!(audio.waveform_peaks.len(), frames.min(512));
            assert_eq!(audio.waveform_colors.len(), audio.waveform_peaks.len());
            assert_eq!(audio.samples.len(), frames);
        }
    }

    #[test]
    fn legacy_layers_default_to_neutral_effects_and_invalid_controls_are_rejected() {
        let layer = SlotSample::new("hit.wav".into(), "hit.wav".into(), None, 1.0);
        let mut value = serde_json::to_value(&layer).unwrap();
        for key in ["pan", "attack_ms", "release_ms"] {
            value.as_object_mut().unwrap().remove(key);
        }
        let mut loaded: SlotSample = serde_json::from_value(value).unwrap();
        assert_eq!(loaded, layer);
        loaded.pan = f32::NAN;
        assert!(loaded.validate().is_err());
        loaded.pan = 0.0;
        loaded.release_ms = 501.0;
        assert!(loaded.validate().is_err());
    }

    #[test]
    fn repeated_saves_preserve_existing_files_and_versions() {
        let root = tempfile::tempdir().unwrap();
        let bank = SamplePackBank::new("Kit");
        let first = save_sample_pack(root.path(), &bank).unwrap();
        std::fs::write(first.join("precious.txt"), b"keep me").unwrap();
        let original = std::fs::read(first.join("bank.json")).unwrap();
        let second = save_sample_pack(root.path(), &bank).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            std::fs::read(first.join("precious.txt")).unwrap(),
            b"keep me"
        );
        assert_eq!(std::fs::read(first.join("bank.json")).unwrap(), original);
        assert!(second.join("bank.json").is_file());
        assert!(save_sample_pack(root.path(), &SamplePackBank::new("..")).is_err());
        assert!(save_sample_pack(root.path(), &SamplePackBank::new(".")).is_err());
    }

    #[test]
    fn linked_pack_destinations_are_never_overwritten_or_deleted() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("precious"), b"untouched").unwrap();
        std::fs::create_dir(root.path().join(PACKS_DIR_NAME)).unwrap();
        let linked = root.path().join(PACKS_DIR_NAME).join("Kit");
        std::os::unix::fs::symlink(outside.path(), &linked).unwrap();
        let saved = save_sample_pack(root.path(), &SamplePackBank::new("Kit")).unwrap();
        assert_ne!(saved, linked);
        assert!(delete_sample_pack(&linked).is_err());
        assert_eq!(
            std::fs::read(outside.path().join("precious")).unwrap(),
            b"untouched"
        );
    }

    #[test]
    fn bank_loading_validates_slot_count_and_audio_bounds() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bank.json");
        let mut bank = SamplePackBank::new("Malformed");
        bank.slots.push(PackSlot::new('x'));
        std::fs::write(&path, serde_json::to_vec(&bank).unwrap()).unwrap();
        assert!(load_sample_pack(root.path()).is_err());
        bank.slots.pop();
        let mut sample = SlotSample::new("missing.wav".into(), "missing.wav".into(), None, 1.0);
        sample.start_sec = 2.0;
        bank.slots[0].samples.push(sample);
        std::fs::write(&path, serde_json::to_vec(&bank).unwrap()).unwrap();
        assert!(load_sample_pack(root.path()).is_err());
        bank.slots[0].samples[0].start_sec = 0.0;
        std::fs::write(&path, serde_json::to_vec(&bank).unwrap()).unwrap();
        let loaded = load_sample_pack(root.path()).unwrap();
        assert!(loaded.slots[0].samples[0].cached_audio.is_none());
        assert!(loaded.slots[0].samples[0].file_path.is_absolute());
    }

    #[test]
    fn shared_pcm_keeps_frame_alignment_and_reuses_storage() {
        let mut sample = SlotSample::new("sample.wav".into(), "sample.wav".into(), None, 1.0);
        let audio = Arc::new(CachedAudio {
            channels: 2,
            sample_rate: 4,
            samples: Arc::new(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
            mono_samples: Arc::new(vec![0.5, 2.5, 4.5, 6.5]),
            waveform_peaks: vec![],
            waveform_colors: vec![],
        });
        sample.cached_audio = Some(audio.clone());
        sample.start_sec = 0.25;
        sample.end_sec = 0.75;
        let source = SharedPcm::trimmed(&sample).unwrap();
        assert!(Arc::ptr_eq(&source.audio, &audio));
        use rodio::Source;
        assert_eq!(source.total_duration(), Some(Duration::from_millis(500)));
        assert_eq!(source.collect::<Vec<_>>(), vec![2.0, 3.0, 4.0, 5.0]);
        sample.end_sec = sample.start_sec;
        assert!(SharedPcm::trimmed(&sample).is_none());
        sample.start_sec = f32::NAN;
        assert!(SharedPcm::trimmed(&sample).is_none());
    }

    #[test]
    fn legacy_banks_extend_without_moving_layers_and_new_settings_round_trip() {
        let mut bank = SamplePackBank::new("Legacy");
        bank.slots.truncate(10);
        bank.slots[9].samples.push(SlotSample::new(
            "tail.wav".into(),
            "tail.wav".into(),
            None,
            1.0,
        ));
        let mut json = serde_json::to_value(&bank).unwrap();
        json.as_object_mut().unwrap().remove("override_velocity");
        json.as_object_mut().unwrap().remove("compressor_bypass");
        json.as_object_mut()
            .unwrap()
            .remove("compressor_threshold_db");
        json.as_object_mut().unwrap().remove("compressor_ratio");
        for slot in json["slots"].as_array_mut().unwrap() {
            slot.as_object_mut().unwrap().remove("choke_group");
        }
        let mut loaded: SamplePackBank = serde_json::from_value(json).unwrap();
        loaded.validate(Path::new("/samples")).unwrap();
        assert_eq!(loaded.slots.len(), 16);
        assert_eq!(
            loaded.slots[9].samples[0].file_path,
            Path::new("/samples/tail.wav")
        );
        assert_eq!(loaded.slots[9].key_char, '0');
        assert!(loaded.slots[10..].iter().all(PackSlot::is_empty));
        assert!(!loaded.override_velocity && !loaded.compressor_bypass);
        loaded.override_velocity = true;
        assert_eq!(
            loaded.compressor_threshold_db,
            default_compressor_threshold()
        );
        assert_eq!(loaded.compressor_ratio, 2.0);
        loaded.compressor_threshold_db = -18.0;
        loaded.compressor_ratio = 6.0;
        loaded.compressor_bypass = true;
        loaded.slots[15].choke_group = 8;
        let mut copy: SamplePackBank =
            serde_json::from_str(&serde_json::to_string(&loaded).unwrap()).unwrap();
        copy.validate(Path::new("/samples")).unwrap();
        assert!(copy.override_velocity && copy.compressor_bypass);
        assert_eq!(copy.compressor_threshold_db, -18.0);
        assert_eq!(copy.compressor_ratio, 6.0);
        copy.compressor_ratio = f32::NAN;
        assert!(copy.validate(Path::new("/samples")).is_err());
        copy.compressor_ratio = 6.0;
        assert_eq!(copy.slots[15].choke_group, 8);
        copy.slots[15].choke_group = 9;
        assert!(copy.validate(Path::new("/samples")).is_err());
    }

    #[test]
    fn test_sample_pack_bank_initialization() {
        let bank = SamplePackBank::new("Test Kit");
        assert_eq!(bank.name, "Test Kit");
        assert_eq!(bank.slots.len(), NUM_SLOTS);
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
            pan: 0.0,
            attack_ms: 0.0,
            release_ms: 0.0,
            cached_audio: None,
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
