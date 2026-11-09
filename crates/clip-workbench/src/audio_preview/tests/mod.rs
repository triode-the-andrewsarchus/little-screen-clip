use super::export::*;
use super::*;
use little_clip_core::audio::compute_full_audio_analysis;

mod analysis;
mod copy;
mod edits;
mod playback;
mod studio;
mod tool_modes;

pub(super) fn render_inspect_frame(
    ctx: &egui::Context,
    state: &mut AudioPreviewState,
    time: f64,
    events: Vec<egui::Event>,
) -> Rect {
    let modifiers = events
        .iter()
        .find_map(|event| match event {
            egui::Event::PointerButton { modifiers, .. }
            | egui::Event::MouseWheel { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or_default();
    render_inspect_frame_with_modifiers(ctx, state, time, events, modifiers)
}

pub(super) fn render_inspect_frame_with_modifiers(
    ctx: &egui::Context,
    state: &mut AudioPreviewState,
    time: f64,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) -> Rect {
    use egui::epaint::Shape;

    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let canvas_fill = crate::hex_to_c32(&palette.colors.surface_recessed);
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 760.0))),
            time: Some(time),
            modifiers,
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut close_requested = false;
                state.render_standard_ui(ui, ctx, &palette, &mut close_requested, None);
            });
        },
    );
    output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Rect(rect)
                if rect.fill == canvas_fill
                    && rect.rounding == egui::Rounding::same(8.0)
                    && rect.rect.width() > 500.0
                    && rect.rect.height() > 100.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("ready Inspect canvas should render")
}

pub(super) fn pointer_button_event(
    pos: Pos2,
    button: egui::PointerButton,
    pressed: bool,
    modifiers: egui::Modifiers,
) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers,
    }
}

pub(super) fn wait_for_audio_save(state: &mut AudioPreviewState) {
    let ctx = egui::Context::default();
    for _ in 0..500 {
        state.poll_save_job(&ctx, 3.0);
        if state.save_job.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("audio save did not finish within five seconds");
}

pub(super) fn wait_for_slice_copy(state: &mut AudioPreviewState) {
    let ctx = egui::Context::default();
    for _ in 0..500 {
        state.poll_slice_copy_job(&ctx, 3.0);
        if state.slice_copy_job.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("audio slice copy did not finish within five seconds");
}

pub(super) fn write_stereo_pcm_fixture(path: &Path, left: &[i16], right: &[i16], sample_rate: u32) {
    use std::io::Write;

    assert_eq!(left.len(), right.len());
    let data_len = (left.len() * 4) as u32;
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36_u32 + data_len).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&2_u16.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&(sample_rate * 4).to_le_bytes()).unwrap();
    file.write_all(&4_u16.to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_len.to_le_bytes()).unwrap();
    for (&left_sample, &right_sample) in left.iter().zip(right) {
        file.write_all(&left_sample.to_le_bytes()).unwrap();
        file.write_all(&right_sample.to_le_bytes()).unwrap();
    }
}

pub(super) fn read_pcm16_wav(path: &Path) -> (u16, u32, Vec<i16>) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut channels = None;
    let mut sample_rate = None;
    let mut samples = None;
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_len =
            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let data_start = offset + 8;
        let data_end = data_start + chunk_len;
        assert!(data_end <= bytes.len(), "WAV chunk extends beyond file");
        match chunk_id {
            b"fmt " => {
                assert_eq!(
                    u16::from_le_bytes(bytes[data_start..data_start + 2].try_into().unwrap()),
                    1,
                    "fixture output should be PCM"
                );
                channels = Some(u16::from_le_bytes(
                    bytes[data_start + 2..data_start + 4].try_into().unwrap(),
                ));
                sample_rate = Some(u32::from_le_bytes(
                    bytes[data_start + 4..data_start + 8].try_into().unwrap(),
                ));
                assert_eq!(
                    u16::from_le_bytes(bytes[data_start + 14..data_start + 16].try_into().unwrap()),
                    16,
                    "fixture output should be 16-bit PCM"
                );
            }
            b"data" => {
                samples = Some(
                    bytes[data_start..data_end]
                        .chunks_exact(2)
                        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
                        .collect(),
                );
            }
            _ => {}
        }
        offset = data_end + (chunk_len % 2);
    }
    (
        channels.expect("WAV fmt chunk"),
        sample_rate.expect("WAV sample rate"),
        samples.expect("WAV data chunk"),
    )
}

pub(super) fn preview_source_with_region(
    region: AudioRegionEdit,
) -> (
    Arc<PlaybackEdits>,
    EditedPreviewSource<rodio::buffer::SamplesBuffer>,
) {
    let shared = Arc::<PlaybackEdits>::default();
    shared.edits.write().unwrap().regions = vec![region];
    shared.generation.fetch_add(1, Ordering::Release);
    let source = rodio::buffer::SamplesBuffer::new(
        std::num::NonZeroU16::new(1).unwrap(),
        std::num::NonZeroU32::new(4).unwrap(),
        vec![0.5; 8],
    );
    let preview = EditedPreviewSource::new(source, Arc::clone(&shared));
    (shared, preview)
}

pub(crate) fn generate_fixture_wav(name: &str, duration_secs: f32) -> (tempfile::TempDir, PathBuf) {
    use std::io::Write;
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join(format!("{name}.wav"));
    let mut file = std::fs::File::create(&path).unwrap();
    let sample_rate = 44100u32;
    let num_samples = (sample_rate as f32 * duration_secs) as usize;
    let num_channels = 2u16;
    let bits_per_sample = 16u16;

    let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
    let block_align = num_channels * (bits_per_sample / 8);
    let data_len = (num_samples * num_channels as usize * 2) as u32;
    let riff_len = 36 + data_len;

    file.write_all(b"RIFF").unwrap();
    file.write_all(&riff_len.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&num_channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&block_align.to_le_bytes()).unwrap();
    file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_len.to_le_bytes()).unwrap();

    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let s1 = ((t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 8000.0) as i16;
        let s2 = ((t * 880.0 * 2.0 * std::f32::consts::PI).sin() * 8000.0) as i16;
        file.write_all(&s1.to_le_bytes()).unwrap();
        file.write_all(&s2.to_le_bytes()).unwrap();
    }
    (temp_dir, path)
}

pub(super) fn empty_audio_analysis() -> FullAudioAnalysis {
    FullAudioAnalysis {
        channels: 2,
        sample_rate: 44100,
        duration_ms: 0,
        total_frames: 0,
        ch1_samples: Vec::new().into(),
        ch2_samples: Some(Vec::new().into()),
        overview_peaks1: Vec::new(),
        overview_peaks2: Some(Vec::new()),
        peaks_lvl0_ch1: Vec::new(),
        peaks_lvl0_ch2: Some(Vec::new()),
        peaks_lvl1_ch1: Vec::new(),
        peaks_lvl1_ch2: Some(Vec::new()),
        peaks_lvl2_ch1: Vec::new(),
        peaks_lvl2_ch2: Some(Vec::new()),
        spectrogram_width: 0,
        spectrogram_height: 0,
        spectrogram_rgba: Vec::new(),
        note_strip_width: 0,
        note_strip_height: 0,
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
    }
}

pub(super) fn wait_for_session_edit(state: &mut AudioPreviewState) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.session_edit_job.is_some() {
        assert!(Instant::now() < deadline, "session edit timed out");
        state.poll_session_edit(&ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
}
