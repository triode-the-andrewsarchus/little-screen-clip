//! Explicit generated audio fixtures for host and studio contract tests.
use std::path::PathBuf;
pub fn generate_fixture_wav(name: &str, duration_secs: f32) -> (tempfile::TempDir, PathBuf) {
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
