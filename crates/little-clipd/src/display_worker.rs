//! Latest-frame delivery and USB session recovery, independent of clipboard updates.

use std::{fmt::Display, sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};
use tracing::{debug, info, warn};

/// Runs all USB callbacks and session destruction on one dedicated owner thread.
/// Closing the frame channel stops the owner after any in-flight driver call returns;
/// transfers retain driver-level timeouts, but OS enumeration is not forcibly
/// interruptible. No async executor thread joins it; process exit releases handles.
pub(super) fn spawn<D: 'static, E: Display + 'static>(
    rx: watch::Receiver<Option<Arc<[u8]>>>,
    open: impl FnMut() -> Result<D, E> + Send + 'static,
    send: impl FnMut(&mut D, &[u8]) -> Result<(), E> + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    std::thread::Builder::new()
        .name("usb-display".into())
        .spawn(move || runtime.block_on(run(None, rx, open, send)))
}

async fn run<D, E: Display>(
    mut driver: Option<D>,
    mut rx: watch::Receiver<Option<Arc<[u8]>>>,
    mut open: impl FnMut() -> Result<D, E>,
    mut send: impl FnMut(&mut D, &[u8]) -> Result<(), E>,
) {
    // The MCU watchdog trips after roughly 1–1.5 seconds of silence.
    let keepalive_interval = Duration::from_millis(400);
    let min_usb_gap = Duration::from_millis(80);
    let retry_interval = Duration::from_millis(500);
    let mut ticker = tokio::time::interval(keepalive_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_send = Instant::now();
    let mut next_connect = Instant::now();

    loop {
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() { break; }
            }
            _ = ticker.tick() => {
                if driver.is_some() && last_send.elapsed() < keepalive_interval {
                    continue;
                }
            }
        }

        if rx.has_changed().is_err() {
            break;
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

        if rx.borrow().is_some() {
            let elapsed = last_send.elapsed();
            if elapsed < min_usb_gap {
                tokio::time::sleep(min_usb_gap - elapsed).await;
            }
            // Opening hardware or pacing a send may take time. Use the newest
            // frame (including a lock-screen blank), never the one from before that wait.
            if rx.has_changed().is_err() {
                break;
            }
            let last_frame = rx.borrow_and_update().clone();
            let (Some(driver_ref), Some(frame)) = (driver.as_mut(), last_frame.as_ref()) else {
                continue;
            };
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
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    struct Device {
        id: usize,
        dropped: Arc<Mutex<Vec<usize>>>,
    }

    impl Drop for Device {
        fn drop(&mut self) {
            self.dropped.lock().unwrap().push(self.id);
        }
    }

    #[tokio::test]
    async fn stalled_hardware_does_not_block_publication_and_releases_on_owner() {
        let caller = std::thread::current().id();
        let (tx, rx) = watch::channel(None);
        let (opening, opened) = tokio::sync::oneshot::channel();
        let (sending, sent) = tokio::sync::oneshot::channel();
        let (resume_open, wait_open) = std::sync::mpsc::channel();
        let (resume_send, wait_send) = std::sync::mpsc::channel();
        let dropped = Arc::new(Mutex::new(Vec::new()));
        let observed_drops = dropped.clone();
        let mut opening = Some(opening);
        let mut sending = Some(sending);
        let worker = spawn(
            rx,
            move || {
                opening
                    .take()
                    .unwrap()
                    .send(std::thread::current().id())
                    .unwrap();
                wait_open.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok::<_, &str>(Device {
                    id: 42,
                    dropped: dropped.clone(),
                })
            },
            move |_, frame| {
                sending
                    .take()
                    .unwrap()
                    .send((std::thread::current().id(), frame.to_vec()))
                    .unwrap();
                wait_send.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(())
            },
        )
        .unwrap();
        let owner = tokio::time::timeout(Duration::from_secs(2), opened)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(owner, caller);
        for value in 0..64 {
            tx.send(Some(Arc::from([value]))).unwrap();
        }
        resume_open.send(()).unwrap();
        let (send_owner, frame) = tokio::time::timeout(Duration::from_secs(2), sent)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner, send_owner);
        assert_eq!(frame, vec![63]);
        // Publication remains bounded and immediate while the native write is stalled.
        for value in 64..128 {
            tx.send(Some(Arc::from([value]))).unwrap();
        }
        drop(tx);
        resume_send.send(()).unwrap();
        tokio::task::spawn_blocking(move || worker.join().unwrap())
            .await
            .unwrap();
        assert_eq!(*observed_drops.lock().unwrap(), vec![42]);
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_disconnects_and_failed_replay_recover_without_new_frames() {
        let dropped = Arc::new(Mutex::new(Vec::new()));
        let initial = Device {
            id: 0,
            dropped: dropped.clone(),
        };
        let (tx, rx) = watch::channel(None);
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

        tx.send(Some(Arc::from([7]))).unwrap();
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
    async fn frame_updates_keep_minimum_gap_and_coalesce_during_pacing() {
        let (tx, rx) = watch::channel(None);
        let (sent, mut frames) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run(
            Some(()),
            rx,
            || Ok::<_, &str>(()),
            move |_, frame| {
                sent.send((frame.to_vec(), Instant::now())).unwrap();
                Ok(())
            },
        ));
        tx.send(Some(Arc::from([1]))).unwrap();
        let first = frames.recv().await.unwrap();
        tx.send(Some(Arc::from([2]))).unwrap();
        tokio::task::yield_now().await;
        tx.send(Some(Arc::from([3]))).unwrap();
        let second = frames.recv().await.unwrap();
        assert_eq!(second.0, vec![3]);
        assert!(second.1 - first.1 >= Duration::from_millis(80));
        drop(tx);
        worker.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_display_retries_at_bounded_rate_and_restores_latest_frame() {
        let (tx, rx) = watch::channel(None);
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

        tx.send(Some(Arc::from([1]))).unwrap();
        let first_open = opens.recv().await.unwrap();
        // New clipboard frames during an outage must not bypass retry pacing.
        tx.send(Some(Arc::from([2]))).unwrap();
        tx.send(Some(Arc::from([3]))).unwrap();
        let second_open = opens.recv().await.unwrap();
        assert!(second_open - first_open >= Duration::from_millis(500));
        assert_eq!(frames.recv().await.unwrap(), vec![3]);
        drop(tx);
        worker.await.unwrap();
    }
}
