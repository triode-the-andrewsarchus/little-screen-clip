use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn preview_state() -> ImagePreviewState {
    let image = image::RgbaImage::from_pixel(42, 28, image::Rgba([40, 80, 120, 255]));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let item = little_clip_core::ClipItem::from_image(bytes, 42, 28, "image/png");
    ImagePreviewState::from_item(&item, &little_clip_core::AppConfig::default()).unwrap()
}

#[test]
fn saturated_shared_queue_can_retry_film_thumbnails_and_cancel_the_retry() {
    let jobs = little_clip_core::BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (_, blocker_rx) = jobs.submit(Duration::from_secs(5), move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, queued_rx) = jobs.submit(Duration::from_secs(5), || ());

    let mut state = preview_state();
    state.run_thumbnail_batch_on(&jobs);
    state.film_sim_state.poll_thumbnail_job_queue();
    assert!(!state.film_sim_state.is_generating_thumbnails);
    assert!(state.film_sim_state.thumbnail_rx.is_none());
    assert!(
        !state
            .film_sim_state
            .thumbnail_retry_ready(std::time::Instant::now())
    );

    release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queued_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert!(!state.retry_thumbnail_batch_if_due(&jobs));
    state.film_sim_state.thumbnail_retry_at =
        Some(std::time::Instant::now() - Duration::from_millis(1));
    assert!(state.retry_thumbnail_batch_if_due(&jobs));
    assert!(state.film_sim_state.is_generating_thumbnails);
    let cancelled = state
        .film_sim_state
        .thumbnail_cancel
        .as_ref()
        .unwrap()
        .clone();
    state.film_sim_state.cancel_thumbnail_batch();
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert!(!state.film_sim_state.is_generating_thumbnails);
    assert!(state.film_sim_state.thumbnail_job_rx.is_none());
}
