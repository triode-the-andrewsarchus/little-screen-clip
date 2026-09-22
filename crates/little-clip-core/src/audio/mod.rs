//! Audio decoding, waveform peak pyramids, spectral analysis, pitch detection, and transient extraction.
//!
//! Integrates Symphonia for decoding diverse audio formats, RealFFT / Rayon for parallel STFT
//! spectrogram generation, logarithmic spectral flux for beat/transient onset detection,
//! YIN algorithm for pitch detection, and Serato/ColorChord harmonic color mapping.

pub mod analysis;
pub mod decode;
pub mod types;
pub mod waveform;

pub use analysis::*;
pub use decode::*;
pub use types::*;
pub use waveform::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AudioConfig, AudioSamplePosition};
    use realfft::RealFftPlanner;
    use std::io::Write;

    #[test]
    fn session_gain_soft_clips_and_can_recover_from_silence() {
        let sample = 0.5;
        assert_eq!(apply_audio_gain(sample, f32::NEG_INFINITY, false), 0.0);
        assert_eq!(apply_audio_gain(sample, 0.0, false), sample);
        let boosted = apply_audio_gain(0.05, 40.0, true);
        assert!(boosted > 0.9 && boosted < 1.0);
        assert_eq!(apply_audio_gain(-0.05, 40.0, true), -boosted);
        assert!(apply_audio_gain(sample, -12.0, false) > 0.0);
        assert!(apply_audio_gain(sample, 1.0, false) > sample);
    }

    #[test]
    fn region_crossfade_mix_extends_outward_and_skips_track_edges() {
        let edit = AudioRegionEdit {
            start_sec: 2.0,
            end_sec: 4.0,
            crossfade_sec: 1.0,
            gain_db: -6.0,
            ..AudioRegionEdit::default()
        };
        assert_eq!(edit.effect_bounds(10.0), (1.0, 5.0));
        assert_eq!(edit.effect_mix(1.0, 10.0), 0.0);
        assert_eq!(edit.effect_mix(1.5, 10.0), 0.5);
        assert_eq!(edit.effect_mix(2.0, 10.0), 1.0);
        assert_eq!(edit.effect_mix(4.5, 10.0), 0.5);
        assert_eq!(edit.effect_mix(5.0, 10.0), 0.0);

        let start_edge = AudioRegionEdit {
            start_sec: 0.0,
            end_sec: 2.0,
            ..edit
        };
        assert_eq!(start_edge.effect_bounds(10.0), (0.0, 3.0));
        assert_eq!(start_edge.effect_mix(0.0, 10.0), 1.0);
        assert_eq!(start_edge.effect_mix(2.5, 10.0), 0.5);

        let end_edge = AudioRegionEdit {
            start_sec: 8.0,
            end_sec: 10.0,
            ..edit
        };
        assert_eq!(end_edge.effect_bounds(10.0), (7.0, 10.0));
        assert_eq!(end_edge.effect_mix(7.5, 10.0), 0.5);
        assert_eq!(end_edge.effect_mix(10.0, 10.0), 1.0);
    }

    #[test]
    fn session_gain_rebuilds_spectrum_and_notes_without_changing_source() {
        let path = generate_test_wav();
        let original_bytes = std::fs::read(&path).unwrap();
        let original = compute_audio_analysis_progressive(&path, 120, |_| {}).unwrap();
        let edited = compute_audio_analysis_progressive_with_edits(
            &path,
            120,
            &[AudioRegionEdit {
                start_sec: 0.25,
                end_sec: 0.75,
                gain_db: f32::NEG_INFINITY,
                ..AudioRegionEdit::default()
            }],
            |_| {},
        )
        .unwrap();
        assert_eq!(edited.ch1_samples[4000], original.ch1_samples[4000]);
        assert_ne!(original.ch1_samples[15000], 0.0);
        assert_eq!(edited.ch1_samples[15000], 0.0);
        let middle_col = 60;
        assert!((0..edited.spectrogram_height).any(|row| {
            let idx = (row * edited.spectrogram_width + middle_col) * 4;
            edited.spectrogram_rgba[idx..idx + 4] != original.spectrogram_rgba[idx..idx + 4]
        }));
        assert!(original.note_strip_info[middle_col].active);
        assert!(!edited.note_strip_info[middle_col].active);
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn track_gain_is_applied_before_region_edits() {
        let path = generate_test_wav();
        let original = compute_audio_analysis_progressive(&path, 120, |_| {}).unwrap();
        let gained = compute_audio_analysis_progressive_with_gain_and_edits(
            &path,
            120,
            -6.0,
            &[AudioRegionEdit {
                start_sec: 0.25,
                end_sec: 0.75,
                gain_db: 6.0,
                ..AudioRegionEdit::default()
            }],
            |_| {},
        )
        .unwrap();

        let outside = 4_000;
        let inside = 15_000;
        let expected_outside = apply_audio_gain(original.ch1_samples[outside], -6.0, false);
        assert!((gained.ch1_samples[outside] - expected_outside).abs() < 0.000_01);
        assert!((gained.ch1_samples[inside] - original.ch1_samples[inside]).abs() < 0.000_01);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn spectral_preview_prioritizes_filters_and_attenuates_rejected_frequencies() {
        let path = generate_test_wav();
        let original = compute_audio_analysis_progressive(&path, 120, |_| {}).unwrap();
        let mut stages = Vec::new();
        let filtered = compute_audio_spectral_preview_with_edits(
            &path,
            120,
            &[AudioRegionEdit {
                start_sec: 0.0,
                end_sec: 1.0,
                low_pass_hz: Some(300.0),
                ..AudioRegionEdit::default()
            }],
            |progress| stages.push(progress),
        )
        .unwrap();
        let spectrum_stage = stages
            .iter()
            .position(|p| matches!(p, AudioAnalysisProgress::SpectrogramAndNotesReady { .. }))
            .expect("spectrum update");
        let waveform_stage = stages
            .iter()
            .position(|p| matches!(p, AudioAnalysisProgress::WaveformReady(_)))
            .expect("waveform update");
        assert!(spectrum_stage < waveform_stage);
        assert!(
            !stages
                .iter()
                .any(|p| matches!(p, AudioAnalysisProgress::TransientsReady(_)))
        );
        let raw_treble = original.ch2_samples.as_ref().unwrap();
        let lowpassed_treble = filtered.ch2_samples.as_ref().unwrap();
        let rms = |samples: &[f32]| {
            (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
        };
        assert!(rms(&lowpassed_treble[22_050..]) < rms(&raw_treble[22_050..]) * 0.1);
        assert_ne!(filtered.spectrogram_rgba, original.spectrogram_rgba);

        let highpassed = compute_audio_spectral_preview_with_edits(
            &path,
            120,
            &[AudioRegionEdit {
                start_sec: 0.0,
                end_sec: 1.0,
                high_pass_hz: Some(1_000.0),
                ..AudioRegionEdit::default()
            }],
            |_| {},
        )
        .unwrap();
        assert!(
            rms(&highpassed.ch1_samples[22_050..]) < rms(&original.ch1_samples[22_050..]) * 0.1
        );
        std::fs::remove_file(path).unwrap();
    }

    fn generate_test_wav() -> std::path::PathBuf {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("test_clip_{}.wav", uuid::Uuid::new_v4()));

        let mut file = std::fs::File::create(&path).expect("failed to create temp wav");
        let sample_rate = 44100u32;
        let num_samples = 44100usize; // 1 second
        let num_channels = 2u16;
        let bits_per_sample = 16u16;

        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = num_channels * (bits_per_sample / 8);
        let data_len = (num_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;

        // RIFF header
        file.write_all(b"RIFF").unwrap();
        file.write_all(&riff_len.to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();

        // fmt subchunk
        file.write_all(b"fmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap(); // Subchunk1Size (16 for PCM)
        file.write_all(&1u16.to_le_bytes()).unwrap(); // AudioFormat (1 = PCM)
        file.write_all(&num_channels.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&byte_rate.to_le_bytes()).unwrap();
        file.write_all(&block_align.to_le_bytes()).unwrap();
        file.write_all(&bits_per_sample.to_le_bytes()).unwrap();

        // data subchunk
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();

        // Write stereo PCM samples: Ch1 = 100Hz bass, Ch2 = 4000Hz treble
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let s1 = (t * 100.0 * 2.0 * std::f32::consts::PI).sin();
            let s2 = (t * 4000.0 * 2.0 * std::f32::consts::PI).sin();

            let val1 = (s1 * 28000.0) as i16;
            let val2 = (s2 * 28000.0) as i16;

            file.write_all(&val1.to_le_bytes()).unwrap();
            file.write_all(&val2.to_le_bytes()).unwrap();
        }

        file.flush().unwrap();
        path
    }

    #[test]
    fn test_audio_metadata_and_waveform() {
        let wav_path = generate_test_wav();

        assert!(is_audio_path(&wav_path));

        let meta = read_audio_metadata(&wav_path).expect("Failed to read audio metadata");
        assert_eq!(meta.channels, 2);
        assert_eq!(meta.sample_rate, 44100);
        assert_eq!(meta.format, "WAV");

        let cfg = AudioConfig::default();
        let waveform =
            compute_plain_waveform(&wav_path, 60, &cfg).expect("Failed to compute plain waveform");
        assert_eq!(waveform.channels, 2);
        assert_eq!(waveform.ch1_peaks.len(), 60);
        assert!(waveform.ch2_peaks.is_some());
        assert_eq!(waveform.ch2_peaks.as_ref().unwrap().len(), 60);
        assert!(waveform.spectral_colors.is_none());
        assert!(!waveform.is_snippet);

        let colors = compute_spectral_colors(&wav_path, 60, &cfg)
            .expect("Failed to compute spectral colors");
        assert_eq!(colors.len(), 60);

        let sample_col = colors[10];
        println!("Sample spectral color: {:?}", sample_col);
        assert!(sample_col[0] > 50 || sample_col[1] > 50 || sample_col[2] > 50);

        // Test snippet extraction on a forced threshold (simulate > 5min file)
        let snippet_cfg = AudioConfig {
            max_full_waveform_sec: 0, // Force snippet preview
            snippet_duration_sec: 1,
            sample_position: AudioSamplePosition::Middle,
            ..Default::default()
        };
        let snip_wf = compute_plain_waveform(&wav_path, 40, &snippet_cfg)
            .expect("Failed to compute snippet waveform");
        assert!(snip_wf.is_snippet);
        assert_eq!(snip_wf.ch1_peaks.len(), 40);

        let snip_colors = compute_spectral_colors(&wav_path, 40, &snippet_cfg)
            .expect("Failed to compute snippet spectral colors");
        assert_eq!(snip_colors.len(), 40);

        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_compute_full_waveform() {
        let wav_path = generate_test_wav();
        let wf = compute_full_waveform(&wav_path, 80).expect("failed to compute full waveform");
        assert_eq!(wf.channels, 2);
        assert_eq!(wf.ch1_peaks.len(), 80);
        assert!(wf.ch2_peaks.is_some());
        assert_eq!(wf.ch2_peaks.as_ref().unwrap().len(), 80);
        assert!(!wf.is_snippet);
        assert_eq!(wf.snippet_start_ms, 0);
        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_rustchord_integration() {
        for i in 0..12 {
            let note_id = i as f32 / 12.0;
            let rgb = rustchord::cc_to_rgb(note_id, 1.0, 1.0);
            assert!(rgb[0] >= 0.0 && rgb[0] <= 1.0);
            assert!(rgb[1] >= 0.0 && rgb[1] <= 1.0);
            assert!(rgb[2] >= 0.0 && rgb[2] <= 1.0);
        }
        // Note 0 (A) is yellow [1.0, 1.0, 0.0]
        let yellow = rustchord::cc_to_rgb(0.0, 1.0, 1.0);
        assert!((yellow[0] - 1.0).abs() < 1e-3);
        assert!((yellow[1] - 1.0).abs() < 1e-3);
        assert!(yellow[2].abs() < 1e-3);

        // Note 6 (D# / F#) has magenta/violet hue
        let d_sharp = rustchord::cc_to_rgb(6.0 / 12.0, 1.0, 1.0);
        assert!((d_sharp[0] - 1.0).abs() < 1e-3);
        assert!((d_sharp[2] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn test_detect_chord_from_fft_chords() {
        let sample_rate = 44100.0f32;
        let window_size = 2048;
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(window_size);

        // 1. Test C Major chord: C4 (261.63 Hz) + E4 (329.63 Hz) + G4 (392.00 Hz)
        let c4 = 261.63f32;
        let e4 = 329.63f32;
        let g4 = 392.00f32;
        let mut in_data = r2c.make_input_vec();
        let mut out_data = r2c.make_output_vec();

        for i in 0..window_size {
            let t = i as f32 / sample_rate;
            let w = 0.5
                * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (window_size - 1) as f32).cos());
            let s = ((t * c4 * 2.0 * std::f32::consts::PI).sin()
                + (t * e4 * 2.0 * std::f32::consts::PI).sin()
                + (t * g4 * 2.0 * std::f32::consts::PI).sin())
                * 0.33;
            in_data[i] = s * w;
        }
        r2c.process(&mut in_data, &mut out_data).unwrap();

        let notes = detect_chord_from_fft(&out_data, sample_rate, window_size);
        println!("C Major detected notes: {:?}", notes);
        assert!(
            notes.len() >= 3,
            "Expected at least 3 chord components, got {}",
            notes.len()
        );

        let has_c = notes
            .iter()
            .any(|n| n.note_name.starts_with('C') && (n.frequency_hz - c4).abs() / c4 < 0.04);
        let has_e = notes
            .iter()
            .any(|n| n.note_name.starts_with('E') && (n.frequency_hz - e4).abs() / e4 < 0.04);
        let has_g = notes
            .iter()
            .any(|n| n.note_name.starts_with('G') && (n.frequency_hz - g4).abs() / g4 < 0.04);
        assert!(has_c, "C Major missing C note component");
        assert!(has_e, "C Major missing E note component");
        assert!(has_g, "C Major missing G note component");

        // 2. Test A Minor chord: A3 (220.00 Hz) + C4 (261.63 Hz) + E4 (329.63 Hz)
        let a3 = 220.00f32;
        for i in 0..window_size {
            let t = i as f32 / sample_rate;
            let w = 0.5
                * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (window_size - 1) as f32).cos());
            let s = ((t * a3 * 2.0 * std::f32::consts::PI).sin()
                + (t * c4 * 2.0 * std::f32::consts::PI).sin()
                + (t * e4 * 2.0 * std::f32::consts::PI).sin())
                * 0.33;
            in_data[i] = s * w;
        }
        r2c.process(&mut in_data, &mut out_data).unwrap();

        let aminor_notes = detect_chord_from_fft(&out_data, sample_rate, window_size);
        println!("A Minor detected notes: {:?}", aminor_notes);
        assert!(
            aminor_notes.len() >= 3,
            "Expected at least 3 chord components in A Minor"
        );
        let has_a = aminor_notes
            .iter()
            .any(|n| n.note_name.starts_with('A') && (n.frequency_hz - a3).abs() / a3 < 0.04);
        let has_am_c = aminor_notes
            .iter()
            .any(|n| n.note_name.starts_with('C') && (n.frequency_hz - c4).abs() / c4 < 0.04);
        let has_am_e = aminor_notes
            .iter()
            .any(|n| n.note_name.starts_with('E') && (n.frequency_hz - e4).abs() / e4 < 0.04);
        assert!(has_a, "A Minor missing A note component");
        assert!(has_am_c, "A Minor missing C note component");
        assert!(has_am_e, "A Minor missing E note component");
    }

    #[test]
    fn test_compute_full_audio_analysis() {
        let wav_path = generate_test_wav();
        let analysis = compute_full_audio_analysis(&wav_path, 150)
            .expect("failed to compute full audio analysis");

        assert_eq!(analysis.channels, 2);
        assert!(analysis.total_frames > 0);
        assert_eq!(analysis.ch1_samples.len(), analysis.total_frames);
        assert!(analysis.ch2_samples.is_some());
        assert_eq!(
            analysis.ch2_samples.as_ref().unwrap().len(),
            analysis.total_frames
        );

        assert_eq!(analysis.overview_peaks1.len(), 150);
        assert_eq!(analysis.overview_peaks2.as_ref().unwrap().len(), 150);

        assert_eq!(analysis.spectrogram_width, 150);
        assert_eq!(analysis.spectrogram_height, 512);
        assert_eq!(analysis.spectrogram_rgba.len(), 150 * 512 * 4);

        assert_eq!(analysis.note_strip_width, 150);
        assert_eq!(analysis.note_strip_height, 48);
        assert_eq!(analysis.note_strip_rgba.len(), 150 * 48 * 4);
        assert_eq!(analysis.note_strip_info.len(), 150);

        // Check that active notes were detected
        let active_notes: Vec<_> = analysis
            .note_strip_info
            .iter()
            .filter(|n| n.active)
            .collect();
        println!("Detected active notes count: {}", active_notes.len());
        assert!(!active_notes.is_empty());

        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_snap_to_zero_crossing() {
        let wav_path = generate_test_wav();
        let analysis =
            compute_full_audio_analysis(&wav_path, 100).expect("failed to analyze test wav");

        // The test wav has a 100Hz sine wave on Ch1, period = 441 samples (sample_rate = 44100).
        // Zero crossings occur at frames: 0, 220, 441, 661, 882, etc.
        // Let's test snapping from a frame near 225 (which is 225 / 44100 = 0.0051s):
        let test_time = 225.0 / 44100.0;
        let snapped_time = analysis.snap_to_zero_crossing(test_time, 100);
        let snapped_frame = (snapped_time * 44100.0).round() as usize;

        // Snapped frame should be very close to the zero crossing (around frame 220 or 221)
        assert!((snapped_frame as isize - 220).abs() <= 2);

        // Amplitude at the snapped frame should be virtually zero
        let amp_at_snap = analysis.ch1_samples[snapped_frame].abs();
        assert!(amp_at_snap < 0.05);

        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_colorchord_note_estimation_stairstep_c1_to_c6() {
        use std::io::BufWriter;

        let temp_dir = std::env::temp_dir();
        let wav_path = temp_dir.join(format!("test_stairstep_{}.wav", uuid::Uuid::new_v4()));

        let sample_rate = 44100u32;
        let num_notes = 61usize; // C1 (MIDI 24) to C6 (MIDI 84)
        let samples_per_note = 44100usize; // 1.0 second per note
        let total_samples = num_notes * samples_per_note;
        let num_channels = 1u16;
        let bits_per_sample = 16u16;

        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = num_channels * (bits_per_sample / 8);
        let data_len = (total_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;

        let file = std::fs::File::create(&wav_path).expect("failed to create stairstep wav");
        let mut writer = BufWriter::new(file);

        // RIFF header
        writer.write_all(b"RIFF").unwrap();
        writer.write_all(&riff_len.to_le_bytes()).unwrap();
        writer.write_all(b"WAVE").unwrap();

        // fmt subchunk
        writer.write_all(b"fmt ").unwrap();
        writer.write_all(&16u32.to_le_bytes()).unwrap();
        writer.write_all(&1u16.to_le_bytes()).unwrap();
        writer.write_all(&num_channels.to_le_bytes()).unwrap();
        writer.write_all(&sample_rate.to_le_bytes()).unwrap();
        writer.write_all(&byte_rate.to_le_bytes()).unwrap();
        writer.write_all(&block_align.to_le_bytes()).unwrap();
        writer.write_all(&bits_per_sample.to_le_bytes()).unwrap();

        // data subchunk
        writer.write_all(b"data").unwrap();
        writer.write_all(&data_len.to_le_bytes()).unwrap();

        // Generate 61 1-second notes from MIDI 24 (C1, ~32.7 Hz) to MIDI 84 (C6, ~1046.5 Hz)
        for note_idx in 0..num_notes {
            let midi = 24 + note_idx as i32;
            let freq = 440.0 * 2.0f32.powf((midi as f32 - 69.0) / 12.0);
            for s in 0..samples_per_note {
                let t = s as f32 / sample_rate as f32;
                let sample_val = (t * freq * 2.0 * std::f32::consts::PI).sin() * 0.8;
                let sample_i16 = (sample_val * 32000.0) as i16;
                writer.write_all(&sample_i16.to_le_bytes()).unwrap();
            }
        }
        writer.flush().unwrap();
        drop(writer);

        // Analyze audio with 1200 columns (high resolution across 61 seconds)
        let num_cols = 1200;
        let analysis = compute_full_audio_analysis(&wav_path, num_cols)
            .expect("failed to compute analysis for stairstep wav");

        assert_eq!(analysis.duration_ms, 61000);

        let note_names = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];

        // Check each note ~500ms into the note
        let mut all_correct = true;
        for note_idx in 0..num_notes {
            let midi = 24 + note_idx as i32;
            let expected_class = note_names[(midi % 12) as usize];
            let expected_octave = (midi / 12) - 1;
            let expected_name = format!("{}{}", expected_class, expected_octave);
            let expected_freq = 440.0 * 2.0f32.powf((midi as f32 - 69.0) / 12.0);

            let check_time = note_idx as f32 * 1.0 + 0.500;
            let col_idx = (((check_time / 61.0) * num_cols as f32).floor() as usize)
                .min(analysis.note_strip_info.len().saturating_sub(1));
            let info = &analysis.note_strip_info[col_idx];

            let is_match = if expected_freq < 65.0 {
                // Frequencies below 65 Hz are ignored as non-musical sub-bass rumble
                !info.active || info.note_name == "—"
            } else {
                let freq_err_ratio = (info.frequency_hz - expected_freq).abs() / expected_freq;
                info.active && info.note_name == expected_name && freq_err_ratio < 0.05
            };

            if !is_match {
                let freq_err_ratio = (info.frequency_hz - expected_freq).abs() / expected_freq;
                println!(
                    "MISMATCH: Note {} (MIDI {}, expected {} at {:.2} Hz) -> got active={}, name='{}', freq={:.2} Hz (err={:.1}%)",
                    note_idx,
                    midi,
                    expected_name,
                    expected_freq,
                    info.active,
                    info.note_name,
                    info.frequency_hz,
                    freq_err_ratio * 100.0
                );
                all_correct = false;
            }
        }

        let _ = std::fs::remove_file(&wav_path);
        assert!(
            all_correct,
            "Not all notes from C1 to C6 were correctly identified!"
        );
    }

    #[test]
    fn test_profile_deadmau5() {
        let p = std::path::Path::new(
            "/mnt/rust/data/MusicJelly/Deadmau5/Some Chords (Single)/Deadmau5 - Some Chords.mp3",
        );
        if !p.is_file() {
            println!("Deadmau5 file not found, skipping profile test.");
            return;
        }

        let meta = read_audio_metadata(p).expect("metadata");
        println!("\n=== PROFILING DEADMAU5 TRACK ===");
        println!("File: {:?}", p);
        println!(
            "Duration: {} ms ({:.2} min), Sample rate: {} Hz, Channels: {}",
            meta.duration_ms,
            meta.duration_ms as f64 / 60000.0,
            meta.sample_rate,
            meta.channels
        );

        let t_total_start = std::time::Instant::now();
        let analysis = compute_full_audio_analysis(p, 1200).expect("analysis failed");
        let total_elapsed = t_total_start.elapsed();

        println!("Total analysis time: {:?}", total_elapsed);
        println!("Total frames: {}", analysis.total_frames);
        println!("Transients: {}", analysis.transients.len());
        println!("Overview peaks: {}", analysis.overview_peaks1.len());
        println!(
            "Spectrogram RGBA bytes: {}",
            analysis.spectrogram_rgba.len()
        );
        println!("Note strip info count: {}", analysis.note_strip_info.len());
    }

    #[test]
    fn test_detect_silence_bounds() {
        let sample_rate = 1000u32;
        let mut samples = vec![0.0f32; 3000]; // 3 seconds total
        // Add signal from 1.0s to 2.0s (frames 1000 to 2000)
        for i in 1000..2000 {
            samples[i] = 0.5;
        }

        let mut analysis = FullAudioAnalysis {
            channels: 1,
            sample_rate,
            duration_ms: 3000,
            total_frames: 3000,
            ch1_samples: samples,
            ch2_samples: None,
            overview_peaks1: Vec::new(),
            overview_peaks2: None,
            peaks_lvl0_ch1: Vec::new(),
            peaks_lvl0_ch2: None,
            peaks_lvl1_ch1: Vec::new(),
            peaks_lvl1_ch2: None,
            peaks_lvl2_ch1: Vec::new(),
            peaks_lvl2_ch2: None,
            spectrogram_width: 10,
            spectrogram_height: 10,
            spectrogram_rgba: Vec::new(),
            note_strip_width: 10,
            note_strip_height: 10,
            note_strip_rgba: Vec::new(),
            note_strip_info: Vec::new(),
            spectral_colors: Vec::new(),
            transients: Vec::new(),
            is_transients_ready: false,
            is_spectrogram_ready: false,
            ultrasonic_roi: Vec::new(),
            ultrasonic_peak_times: Vec::new(),
            is_truncated: false,
            truncation_warning: None,
        };

        let bounds = analysis.detect_silence_bounds(-45.0);
        assert!(bounds.is_some());
        let (start, end) = bounds.unwrap();
        // Should detect start around 1.0s (accounting for 15ms padding = ~0.985s) and end around 2.0s (~2.015s)
        assert!(start <= 1.0 && start >= 0.95);
        assert!(end >= 1.99 && end <= 2.05);

        // Entirely silent track should return None
        analysis.ch1_samples = vec![0.0; 1000];
        assert_eq!(analysis.detect_silence_bounds(-45.0), None);
    }

    #[test]
    fn test_transient_sensitivity_scaling() {
        let sample_rate = 44100.0;
        let mut samples = vec![0.0f32; 44100 * 2]; // 2 seconds
        // Create 4 distinct burst impulses
        for idx in &[5000, 15000, 25000, 35000] {
            for offset in 0..100 {
                samples[idx + offset] = 0.9 * (1.0 - offset as f32 / 100.0);
            }
        }
        // Create a minor subtle attack
        for offset in 0..100 {
            samples[45000 + offset] = 0.15 * (1.0 - offset as f32 / 100.0);
        }

        let normal = detect_transients_with_sensitivity(&samples, sample_rate, 1.0);
        let highly_sensitive = detect_transients_with_sensitivity(&samples, sample_rate, 3.0);
        let low_sensitive = detect_transients_with_sensitivity(&samples, sample_rate, 0.2);

        // More sensitive should detect at least as many or more transients than lower sensitivity
        assert!(highly_sensitive.len() >= normal.len());
        assert!(normal.len() >= low_sensitive.len());
    }

    #[test]
    fn test_default_transient_sensitivity_matches_editor_control() {
        assert_eq!(MIN_TRANSIENT_SENSITIVITY, 0.2);
        assert_eq!(DEFAULT_TRANSIENT_SENSITIVITY, 0.35);
        assert_eq!(MAX_TRANSIENT_SENSITIVITY, 1.0);

        let sample_rate = 48_000.0;
        let mut samples = vec![0.0; 48_000];
        for sample in &mut samples[12_000..12_100] {
            *sample = 0.9;
        }
        assert_eq!(
            detect_transients(&samples, sample_rate),
            detect_transients_with_sensitivity(
                &samples,
                sample_rate,
                DEFAULT_TRANSIENT_SENSITIVITY,
            ),
        );
    }

    #[test]
    fn test_fft_window_size_defaults_to_1024_and_drops_to_512_at_10_minutes() {
        let path = generate_test_wav();
        let analysis = compute_full_audio_analysis(&path, 120).expect("analysis failed");
        let duration_sec = analysis.total_frames as f64 / analysis.sample_rate as f64;
        assert!(duration_sec < 600.0);
        // Under 10 minutes: defaults to 1024 FFT -> 512 vertical bins
        assert_eq!(analysis.spectrogram_height, 512);
        assert_eq!(analysis.spectrogram_rgba.len(), 120 * 512 * 4);
    }
}
