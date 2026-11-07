//! Folder-local note families derived on snapshot workers, including cached indexes.
//! Explicit filename notes take precedence over acoustic estimates. Velocity,
//! articulation and round-robin suffixes remain in the family key.
use super::db::ConstellationSample;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct SampleSet {
    pub label: String,
    /// Sorted by recorded note, then original path for deterministic ties.
    pub members: Vec<usize>,
    pub representative: usize,
    pub center: egui::Pos2,
}

#[derive(Clone, Debug, Default)]
pub struct SampleSets {
    pub sets: Vec<SampleSet>,
    pub membership: Vec<Option<usize>>,
    pub notes: Vec<Option<f32>>,
    indices: HashMap<PathBuf, usize>,
}

impl SampleSets {
    /// Rebuild from the complete snapshot, never from a filtered/recursive display subset.
    pub fn build(samples: &[ConstellationSample]) -> Self {
        let mut result = Self {
            membership: vec![None; samples.len()],
            notes: Vec::with_capacity(samples.len()),
            sets: Vec::new(),
            indices: samples
                .iter()
                .enumerate()
                .map(|(index, sample)| (sample.file_path.clone(), index))
                .collect(),
        };
        let mut families = BTreeMap::<(PathBuf, String), Vec<usize>>::new();
        for (index, sample) in samples.iter().enumerate() {
            let named = filename_note(&sample.file_path);
            result.notes.push(
                named
                    .as_ref()
                    .map(|(_, note)| *note as f32)
                    .or_else(|| acoustic_note(sample)),
            );
            if let Some((family, _)) = named {
                families
                    .entry((
                        sample
                            .file_path
                            .parent()
                            .unwrap_or(Path::new(""))
                            .to_owned(),
                        family,
                    ))
                    .or_default()
                    .push(index);
            }
        }
        for ((folder, family), mut members) in families {
            members.sort_by(|a, b| {
                result.notes[*a]
                    .partial_cmp(&result.notes[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| samples[*a].file_path.cmp(&samples[*b].file_path))
            });
            if members.len() < 2
                || result.notes[members[0]] == result.notes[*members.last().unwrap()]
            {
                continue;
            }
            let representative = members[(members.len() - 1) / 2];
            let center = members.iter().fold(egui::Vec2::ZERO, |sum, index| {
                sum + egui::vec2(samples[*index].x, samples[*index].y)
            }) / members.len() as f32;
            let set_index = result.sets.len();
            for &index in &members {
                result.membership[index] = Some(set_index);
            }
            let label = if family.is_empty() {
                folder
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                family
            };
            result.sets.push(SampleSet {
                label,
                members,
                representative,
                center: center.to_pos2(),
            });
        }
        result
    }

    pub fn index_for_path(&self, path: &Path) -> Option<usize> {
        self.indices.get(path).copied()
    }

    pub fn set_for(&self, index: usize) -> Option<usize> {
        self.membership.get(index).copied().flatten()
    }

    pub fn note(&self, index: usize) -> Option<f32> {
        self.notes.get(index).copied().flatten()
    }
}

/// Reject old range-boundary artifacts consistently in the map, filters and hover HUD.
pub(super) fn acoustic_note(sample: &ConstellationSample) -> Option<f32> {
    let modern = sample
        .source_revision
        .as_ref()
        .is_some_and(|revision| revision.ends_with(super::classifier::PITCH_ANALYSIS_REVISION));
    (sample.pitch_clarity >= 0.65
        && sample.pitch_hz.is_finite()
        && sample.pitch_hz > 0.0
        && (modern || !(950.0..=1050.0).contains(&sample.pitch_hz)))
    .then(|| 69.0 + 12.0 * (sample.pitch_hz / 440.0).log2())
    .filter(|note| (0.0..=127.0).contains(note))
}

/// Explicit octave notes override detection, even for a single recording.
/// Preserve the entire remaining name as the family, including articulation/velocity.
/// An empty family is reserved for bare note names within one actual folder.
/// Multiple note tokens (ranges/chords) are deliberately ambiguous and stay ungrouped.
pub(super) fn filename_note(path: &Path) -> Option<(String, u8)> {
    static NOTE: OnceLock<regex::Regex> = OnceLock::new();
    let regex = NOTE.get_or_init(|| {
        regex::Regex::new(r"(?i)(?:^|[_ .()\[\]-])([a-g])([#b]?)(-?[0-9])(?:$|[_ .()\[\]-])")
            .unwrap()
    });
    let stem = path.file_stem()?.to_str()?;
    // Synth700 uses f0/f2/f3/f4/f6 as bank IDs before the waveform name,
    // e.g. f4-saw-C4-127. Keep that bank in the family, not as a second pitch.
    static BANK: OnceLock<regex::Regex> = OnceLock::new();
    let bank =
        BANK.get_or_init(|| regex::Regex::new(r"^f[0-9]-(?:saw|sqr|tri|chor[0-9]+)-").unwrap());
    let offset = bank.find(stem).map_or(0, |bank| bank.end());
    let search = &stem[offset..];
    static ATTACHED: OnceLock<regex::Regex> = OnceLock::new();
    // Uppercase attached notes occur in e.g. DoubleSawsC3. Lowercase endings
    // would misread numbered names such as snare2 and ride3 in the library.
    let attached = ATTACHED.get_or_init(|| regex::Regex::new(r"([A-G])([#b]?)(-?[0-9])$").unwrap());
    let capture = regex
        .captures(search)
        .or_else(|| attached.captures(search))?;
    let start = offset + capture.get(1)?.start();
    let end = offset + capture.get(3)?.end();
    // Search again from the note end so a shared separator cannot hide another note.
    if regex.is_match(&stem[end..]) {
        return None;
    }
    let semitone = match capture[1].to_ascii_uppercase().as_str() {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    } + match capture[2].to_ascii_lowercase().as_str() {
        "#" => 1,
        "b" => -1,
        _ => 0,
    };
    let octave: i32 = capture[3].parse().ok()?;
    let note = (octave + 1) * 12 + semitone;
    if !(0..=127).contains(&note) {
        return None;
    }
    let family = format!("{}{}", &stem[..start], &stem[end..]);
    let family = family
        .split(['_', ' ', '.', '-', '(', ')', '[', ']'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    Some((family, note as u8))
}

/// Bounded power curve: long samples grow sublinearly and very short hits remain visible.
pub(super) fn duration_radius(duration: f32) -> f32 {
    if !duration.is_finite() {
        return 1.0;
    }
    (duration.max(0.001) / 0.5).powf(0.22).clamp(0.65, 1.8)
}

/// C4 is the reference plane; unknown pitches stay on it, without implying a low note.
pub(super) fn pitch_height(note: Option<f32>) -> f32 {
    note.map_or(0.0, |note| ((note - 60.0) / 12.0) * 0.11)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_notes_preserve_articulation_and_reject_ambiguous_names() {
        for (name, family, note) in [
            ("old_piano_C4.wav", "old_piano", 60),
            ("old_piano_Db3.wav", "old_piano", 49),
            ("Piano-F#2-vel80.wav", "Piano_vel80", 42),
            ("C4.wav", "", 60),
            ("123_C4.wav", "123", 60),
            ("old_piano_C0.wav", "old_piano", 12),
            ("pianoC4.wav", "piano", 60),
            ("DoubleSawsC3.wav", "DoubleSaws", 48),
            ("f4-saw-C4-127.wav", "f4_saw_127", 60),
            ("f0-chor1-A2-114.wav", "f0_chor1_114", 45),
            ("C4_Piano.wav", "Piano", 60),
            ("Piano[C4].wav", "Piano", 60),
            ("Piano C-1.wav", "Piano", 0),
        ] {
            assert_eq!(filename_note(Path::new(name)), Some((family.into(), note)));
        }
        for name in [
            "piano_64.wav",
            "piano_C.wav",
            "snare2.wav",
            "ride3.wav",
            "piano_C4_D4.wav",
            "piano_A9.wav",
            "piano_C4-D5.wav",
        ] {
            assert!(filename_note(Path::new(name)).is_none(), "{name}");
        }
    }

    #[test]
    fn duration_curve_and_pitch_reference_are_bounded() {
        assert_eq!(duration_radius(0.0), 0.65);
        assert_eq!(duration_radius(0.5), 1.0);
        assert_eq!(duration_radius(10000.0), 1.8);
        assert!(duration_radius(0.1) < duration_radius(1.0));
        assert_eq!(pitch_height(None), 0.0);
        assert_eq!(pitch_height(Some(60.0)), 0.0);
        assert!((pitch_height(Some(72.0)) - 0.11).abs() < 0.00001);
        assert!(pitch_height(Some(48.0)) < 0.0);
    }
}
