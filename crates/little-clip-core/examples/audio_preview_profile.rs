//! Compare the actual streaming Files renderers and separate Dadaspec feed/finish.
//!
//! Run `cargo run -p little-clip-core --example audio_preview_profile --
//! 60 48000 5 all`. Add `--release` for deployment timings. The final argument
//! can be `waveform`, `dadaspec`, or `analysis` for sampling with perf. An optional
//! fifth argument supplies an ordinary audio file (or `-` for the generated WAV);
//! a sixth argument sets the strip width in physical pixels (default 512).
use little_clip_core::audio::{compute_preview_waveform, dadaspec::DadaspecBuilder};
use std::{hint::black_box, path::Path, sync::atomic::AtomicBool, time::Instant};

fn signal(seconds: usize, rate: u32) -> (Vec<f32>, Vec<f32>) {
    let mut noise = 1_u32;
    let mut left = Vec::with_capacity(seconds * rate as usize);
    let mut right = Vec::with_capacity(left.capacity());
    for index in 0..seconds * rate as usize {
        noise = noise.wrapping_mul(1664525).wrapping_add(1013904223);
        let random = (noise >> 8) as f32 / 16777216.0 - 0.5;
        let t = index as f64 / rate as f64;
        let envelope = (1.0 - t.fract()).powi(2) as f32;
        left.push(
            envelope * (0.5 * (t * std::f64::consts::TAU * 80.0).sin() as f32 + random * 0.1),
        );
        right.push(
            envelope * (0.4 * (t * std::f64::consts::TAU * 3700.0).sin() as f32 + random * 0.1),
        );
    }
    (left, right)
}

fn write_fixture(path: &Path, left: &[f32], right: &[f32], rate: u32) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for (&left, &right) in left.iter().zip(right) {
        writer.write_sample((left * 32767.0) as i16).unwrap();
        writer.write_sample((right * 32767.0) as i16).unwrap();
    }
    writer.finalize().unwrap();
}

fn report(mode: &str, case: &str, seconds: usize, rate: u32, width: u32, mut times: Vec<f64>) {
    times.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "mode": mode, "case": case, "seconds": seconds, "rate": rate,
            "debug_assertions": cfg!(debug_assertions), "width": width, "runs": times.len(),
            "median_ms": times[times.len() / 2], "min_ms": times[0], "all_ms": times,
        })
    );
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let seconds: usize = args.first().map_or("60", String::as_str).parse().unwrap();
    let rate: u32 = args.get(1).map_or("48000", String::as_str).parse().unwrap();
    let runs: usize = args.get(2).map_or("5", String::as_str).parse().unwrap();
    let mode = args.get(3).map_or("all", String::as_str);
    let width: u32 = args.get(5).map_or("512", String::as_str).parse().unwrap();
    assert!((1..=4096).contains(&width));
    assert!((1..=600).contains(&seconds) && (8000..=192000).contains(&rate) && runs > 0);
    assert!(["all", "waveform", "dadaspec", "analysis"].contains(&mode));
    let (left, right) = signal(seconds, rate);
    let temp = tempfile::tempdir().unwrap();
    let fixture = temp.path().join("signal.wav");
    let path = if let Some(path) = args.get(4).filter(|path| path.as_str() != "-") {
        Path::new(path)
    } else {
        write_fixture(&fixture, &left, &right, rate);
        &fixture
    };
    let duration = little_clip_core::audio::read_audio_metadata(path)
        .unwrap()
        .duration_ms as f64
        / 1000.0;
    let cancel = AtomicBool::new(false);
    for case in ["waveform", "dadaspec"] {
        if mode != "all" && mode != case {
            continue;
        }
        let mut times = Vec::new();
        for run in 0..=runs {
            let file = little_clip_core::kernel_interface::open_content(path, true).unwrap();
            let started = Instant::now();
            match case {
                "waveform" => {
                    black_box(
                        compute_preview_waveform(file, path, duration as f32, 512, None, &cancel)
                            .unwrap(),
                    );
                }
                _ => {
                    black_box(
                        little_clip_core::audio::dadaspec::render_file(
                            file, path, 0.0, duration, width, 60, &cancel,
                        )
                        .unwrap(),
                    );
                }
            }
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            if run > 0 {
                times.push(elapsed);
            }
        }
        report(mode, case, seconds, rate, width, times);
    }
    if mode == "all" || mode == "analysis" {
        let mut feeds = Vec::new();
        let mut finishes = Vec::new();
        for run in 0..=runs {
            let mut builder = DadaspecBuilder::new(left.len(), rate).unwrap();
            let started = Instant::now();
            for (left, right) in left.chunks(2048).zip(right.chunks(2048)) {
                builder.push_planar(left, Some(right));
            }
            let feed = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            black_box(builder.finish(width, 60, &cancel).unwrap());
            let finish = started.elapsed().as_secs_f64() * 1000.0;
            if run > 0 {
                feeds.push(feed);
                finishes.push(finish);
            }
        }
        report(mode, "dadaspec_feed", seconds, rate, width, feeds);
        report(mode, "dadaspec_finish", seconds, rate, width, finishes);
    }
}
