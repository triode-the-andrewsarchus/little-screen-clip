//! Latest-frame delivery and USB session recovery, independent of clipboard updates.

use std::{fmt::Display, time::Duration};
use tokio::{sync::mpsc, time::Instant};
use tracing::{debug, info, warn};

pub(super) async fn run<D, E: Display>(
    mut driver: Option<D>,
    mut rx: mpsc::Receiver<Vec<u8>>,
    mut open: impl FnMut() -> Result<D, E>,
    mut send: impl FnMut(&mut D, &[u8]) -> Result<(), E>,
) {
    // The MCU watchdog trips after roughly 1–1.5 seconds of silence.
    let keepalive_interval = Duration::from_millis(400);
    let min_usb_gap = Duration::from_millis(80);
    let retry_interval = Duration::from_millis(500);
    let mut ticker = tokio::time::interval(keepalive_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_frame = None;
    let mut last_send = Instant::now();
    let mut next_connect = Instant::now();

    loop {
        tokio::select! {
            frame = rx.recv() => {
                let Some(mut frame) = frame else { break };
                while let Ok(freshest) = rx.try_recv() {
                    frame = freshest;
                }
                last_frame = Some(frame);
            }
            _ = ticker.tick() => {
                if driver.is_some() && last_send.elapsed() < keepalive_interval {
                    continue;
                }
            }
        }

        if driver.is_none() {
            if Instant::now() < next_connect {
                continue;
            }
            match open() {
                Ok(new_driver) => {
                    info!("Connected to Trofeo USB display; restoring latest frame");
                    driver = Some(new_driver);
                }
                Err(error) => {
                    debug!("Trofeo display not ready: {error}");
                    next_connect = Instant::now() + retry_interval;
                    continue;
                }
            }
        }

        if let (Some(driver_ref), Some(frame)) = (driver.as_mut(), last_frame.as_ref()) {
            let elapsed = last_send.elapsed();
            if elapsed < min_usb_gap {
                tokio::time::sleep(min_usb_gap - elapsed).await;
            }
            match send(driver_ref, frame) {
                Ok(()) => last_send = Instant::now(),
                Err(error) => {
                    warn!("USB display error: {error}. Retrying connection...");
                    // Release interface 0 before any further open/handshake attempt.
                    driver = None;
                    next_connect = Instant::now() + retry_interval;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Device {
        id: usize,
        dropped: Arc<Mutex<Vec<usize>>>,
    }

    impl Drop for Device {
        fn drop(&mut self) {
            self.dropped.lock().unwrap().push(self.id);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_disconnects_and_failed_replay_recover_without_new_frames() {
        let dropped = Arc::new(Mutex::new(Vec::new()));
        let initial = Device {
            id: 0,
            dropped: dropped.clone(),
        };
        let (tx, rx) = mpsc::channel(16);
        let (events, mut attempts) = mpsc::unbounded_channel();
        let mut opens = 0;
        let worker = tokio::spawn(run(
            Some(initial),
            rx,
            move || {
                opens += 1;
                // The old session must release the interface before reopening.
                assert!(dropped.lock().unwrap().contains(&0));
                if opens >= 4 {
                    assert!(dropped.lock().unwrap().contains(&1));
                }
                match opens {
                    1 | 2 => Err("enumeration/handshake failed"),
                    _ => Ok(Device {
                        id: opens - 2,
                        dropped: dropped.clone(),
                    }),
                }
            },
            move |device, frame| {
                events
                    .send((device.id, frame.to_vec(), Instant::now()))
                    .unwrap();
                if device.id < 2 {
                    Err("transport failed")
                } else {
                    Ok(())
                }
            },
        ));

        tx.send(vec![7]).await.unwrap();
        let first = attempts.recv().await.unwrap();
        assert_eq!((first.0, first.1), (0, vec![7]));
        let replay = attempts.recv().await.unwrap();
        assert_eq!((replay.0, replay.1), (1, vec![7]));
        let recovered = attempts.recv().await.unwrap();
        assert_eq!((recovered.0, recovered.1), (2, vec![7]));
        assert!(recovered.2 - replay.2 >= Duration::from_millis(500));
        let keepalive = attempts.recv().await.unwrap();
        assert_eq!((keepalive.0, keepalive.1), (2, vec![7]));
        assert!(keepalive.2 - recovered.2 >= Duration::from_millis(400));
        drop(tx);
        worker.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_display_retries_at_bounded_rate_and_restores_latest_frame() {
        let (tx, rx) = mpsc::channel(16);
        let (opened, mut opens) = mpsc::unbounded_channel();
        let (sent, mut frames) = mpsc::unbounded_channel();
        let mut count = 0;
        let worker = tokio::spawn(run(
            None,
            rx,
            move || {
                count += 1;
                opened.send(Instant::now()).unwrap();
                if count == 1 {
                    Err("not enumerated")
                } else {
                    Ok(())
                }
            },
            move |_, frame| {
                sent.send(frame.to_vec()).unwrap();
                Ok(())
            },
        ));

        tx.send(vec![1]).await.unwrap();
        let first_open = opens.recv().await.unwrap();
        // New clipboard frames during an outage must not bypass retry pacing.
        tx.send(vec![2]).await.unwrap();
        tx.send(vec![3]).await.unwrap();
        let second_open = opens.recv().await.unwrap();
        assert!(second_open - first_open >= Duration::from_millis(500));
        assert_eq!(frames.recv().await.unwrap(), vec![3]);
        drop(tx);
        worker.await.unwrap();
    }
}
