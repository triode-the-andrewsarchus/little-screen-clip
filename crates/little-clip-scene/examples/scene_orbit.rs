//! Optimized end-to-end scene timings and exported orbit frames for visual inspection.

use little_clip_scene::{SplatRenderer, SplatScene};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or("Usage: scene_orbit <scene> <output-directory>")?,
    );
    let output = PathBuf::from(args.next().ok_or("Missing output directory")?);
    std::fs::create_dir_all(&output)?;
    let started = Instant::now();
    let scene = SplatScene::load(&path)?;
    let points = scene.len();
    let loaded = started.elapsed();
    let mut renderer = SplatRenderer::new(Arc::new(scene))?;
    let uploaded = started.elapsed();
    let orbit = renderer.bake_orbit(480, 320, || false)?;
    let baked = started.elapsed();
    drop(renderer);
    let playback = Instant::now();
    let mut checksum = 0u64;
    for frame in 0..10_000 {
        checksum = checksum.wrapping_add(u64::from(
            orbit.frame_at(Duration::from_millis(frame * 400))[(240, 160)][0],
        ));
    }
    let playback = playback.elapsed();
    orbit.frames()[0].save(output.join("orbit-00.png"))?;
    orbit.frames()[15].save(output.join("orbit-15.png"))?;
    println!(
        "{}: {points} splats; load={loaded:?}; upload={:?}; bake30={:?}; total={baked:?}; CPU playback10000={playback:?}; checksum={checksum}",
        path.display(),
        uploaded - loaded,
        baked - uploaded
    );
    Ok(())
}
