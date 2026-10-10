//! Optimized end-to-end scene timings and exported orbit frames for visual inspection.

use little_clip_scene::{ORBIT_FRAME_COUNT, OrbitCamera, OrbitLoop, SplatRenderer, SplatScene};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or("Usage: scene_orbit <scene> <output-directory> [--strip|--hover]")?,
    );
    let output = PathBuf::from(args.next().ok_or("Missing output directory")?);
    let size = match args.next().as_deref() {
        Some(value) if value == std::ffi::OsStr::new("--strip") => [192, 108],
        Some(value) if value == std::ffi::OsStr::new("--hover") => [640, 360],
        _ => [480, 320],
    };
    benchmark(&path, &output, size)
}

fn benchmark(
    path: &std::path::Path,
    output: &std::path::Path,
    size: [u32; 2],
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&output)?;
    let started = Instant::now();
    let scene = SplatScene::load(path)?;
    let points = scene.len();
    let loaded = started.elapsed();
    let mut renderer = SplatRenderer::new(Arc::new(scene))?;
    let uploaded = started.elapsed();
    let (first_image, first, live) = measure_live(&mut renderer, size)?;
    first_image.save(output.join("first-view.png"))?;
    let before_bake = started.elapsed();
    let orbit = renderer.bake_orbit(size[0], size[1], || false)?;
    let baked = started.elapsed();
    drop(renderer);
    let (playback, decode, checksum) = measure_playback(&orbit)?;
    orbit.read_frame(0)?.save(output.join("orbit-00.png"))?;
    orbit
        .read_frame(ORBIT_FRAME_COUNT / 2)?
        .save(output.join("orbit-opposite.png"))?;
    println!(
        "{}: {points} splats; load={loaded:?}; upload={:?}; first-frame={:?}; live20={live:?}; live-average={:?}; bake240={:?}; total={baked:?}; CPU index10000={playback:?}; decode240={decode:?}; temporary bytes={}; checksum={checksum}",
        path.display(),
        uploaded - loaded,
        first,
        live / 20,
        baked - before_bake,
        orbit.disk_bytes()
    );
    Ok(())
}

fn measure_live(
    renderer: &mut SplatRenderer,
    size: [u32; 2],
) -> Result<(image::RgbaImage, Duration, Duration), little_clip_scene::SceneError> {
    let first = Instant::now();
    let image = renderer.render(OrbitCamera::default(), size[0], size[1])?;
    let first = first.elapsed();
    let live = Instant::now();
    for index in 0..20 {
        renderer.render(
            OrbitCamera {
                yaw: index as f32 * std::f32::consts::TAU / 20.0,
                ..Default::default()
            },
            size[0],
            size[1],
        )?;
    }
    Ok((image, first, live.elapsed()))
}

fn measure_playback(
    orbit: &OrbitLoop,
) -> Result<(Duration, Duration, u64), little_clip_scene::SceneError> {
    let playback = Instant::now();
    let mut checksum = 0u64;
    for frame in 0..10_000 {
        checksum = checksum.wrapping_add(u64::from(OrbitLoop::frame_index(Duration::from_millis(
            frame * 50,
        )) as u8));
    }
    let playback = playback.elapsed();
    let decode = Instant::now();
    for index in 0..ORBIT_FRAME_COUNT {
        checksum = checksum.wrapping_add(u64::from(orbit.read_frame(index)?[(0, 0)][0]));
    }
    let decode = decode.elapsed();
    Ok((playback, decode, checksum))
}
