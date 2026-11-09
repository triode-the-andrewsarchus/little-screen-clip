//! Background audio ring buffer capture worker.
//!
//! Spawns `pw-record` (or `parec` fallback) to stream raw PCM audio from the system default sink
//! into a shared [`RollingAudioBuffer`].

use little_clip_core::config::AudioConfig;
use little_clip_core::rolling_audio::RollingAudioBuffer;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{info, warn};

/// Spawns a background worker thread continuously capturing system audio into `buffer`.
pub fn start_rolling_capture_thread(
    buffer: Arc<RwLock<RollingAudioBuffer>>,
    config: Arc<RwLock<AudioConfig>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("rolling-audio-capture".into())
        .spawn(move || {
            run_capture_loop(buffer, config);
        })
        .expect("Failed to spawn rolling audio capture thread")
}

fn spawn_capture_child(target_device: Option<&str>) -> Option<Child> {
    let target = target_device.unwrap_or("@DEFAULT_SINK@");
    // Try pw-record first (PipeWire native)
    let pw_res = Command::new("pw-record")
        .args([
            "--raw",
            "--target",
            target,
            "-P",
            "stream.capture.sink=true",
            "--rate",
            "48000",
            "--channels",
            "2",
            "--format",
            "s16",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    match pw_res {
        Ok(child) => {
            info!(
                "Spawned pw-record for system audio capture on target '{}'",
                target
            );
            return Some(child);
        }
        Err(e) => {
            warn!("Failed to spawn pw-record: {}. Trying parec fallback...", e);
        }
    }

    // Fallback: parec (PulseAudio compatibility)
    let pa_dev = if let Some(d) = target_device {
        if d.ends_with(".monitor") {
            d.to_string()
        } else {
            format!("{}.monitor", d)
        }
    } else {
        "@DEFAULT_SINK@.monitor".to_string()
    };
    let pa_res = Command::new("parec")
        .args([
            "-d",
            &pa_dev,
            "--rate",
            "48000",
            "--channels",
            "2",
            "--format",
            "s16le",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    match pa_res {
        Ok(child) => {
            info!("Spawned parec for system audio capture on '{}'", pa_dev);
            Some(child)
        }
        Err(e) => {
            warn!("Failed to spawn parec: {}", e);
            None
        }
    }
}

const CAPTURE_FRAME_BYTES: usize = 4; // 2 channels * 2 bytes (s16le)

fn read_capture_chunk<R: Read>(
    stdout: &mut R,
    buf: &mut [u8; 4096],
    carry: &mut usize,
    buffer: &Arc<RwLock<RollingAudioBuffer>>,
) -> std::io::Result<usize> {
    let n = stdout.read(&mut buf[*carry..])?;
    if n == 0 {
        return Ok(0);
    }
    let total = *carry + n;
    let aligned = total - (total % CAPTURE_FRAME_BYTES);
    if aligned > 0
        && let Ok(mut lock) = buffer.write()
    {
        lock.push_pcm_s16le(&buf[..aligned]);
    }
    *carry = total - aligned;
    if *carry > 0 {
        buf.copy_within(aligned..total, 0);
    }
    Ok(n)
}

fn run_capture_loop(buffer: Arc<RwLock<RollingAudioBuffer>>, config: Arc<RwLock<AudioConfig>>) {
    let mut buf = [0u8; 4096];

    loop {
        let active_config = match config.read() {
            Ok(config) => config.clone(),
            Err(_) => {
                warn!("Rolling audio configuration lock was poisoned; retrying");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if !active_config.rolling_capture_enabled {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        let dev_str = active_config
            .rolling_capture_device
            .as_deref()
            .unwrap_or("@DEFAULT_SINK@");
        info!(
            "Starting background rolling audio capture ({}s buffer on '{}')",
            active_config.rolling_capture_duration_sec, dev_str
        );

        let mut child = match spawn_capture_child(active_config.rolling_capture_device.as_deref()) {
            Some(c) => c,
            None => {
                warn!("Unable to start audio capture process. Retrying in 5 seconds...");
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };

        if let Some(mut stdout) = child.stdout.take() {
            let mut carry = 0usize;
            loop {
                match read_capture_chunk(&mut stdout, &mut buf, &mut carry, &buffer) {
                    Ok(0) => {
                        warn!("Audio capture process reached EOF (process exited)");
                        break;
                    }
                    Ok(_) => {
                        if capture_settings_changed(&active_config, &config) {
                            info!(
                                "Rolling audio capture settings changed; restarting capture process"
                            );
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Error reading from audio capture stdout: {}", e);
                        break;
                    }
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();

        info!("Restarting audio capture worker in 2 seconds...");
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn capture_settings_changed(active: &AudioConfig, current: &Arc<RwLock<AudioConfig>>) -> bool {
    match current.read() {
        Ok(current) => {
            active.rolling_capture_enabled != current.rolling_capture_enabled
                || active.rolling_capture_device != current.rolling_capture_device
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_restarts_when_device_or_enabled_state_changes() {
        let active = AudioConfig::default();
        let current = Arc::new(RwLock::new(active.clone()));
        assert!(!capture_settings_changed(&active, &current));

        current.write().unwrap().rolling_capture_enabled = false;
        assert!(capture_settings_changed(&active, &current));
    }

    #[test]
    fn partial_pipe_reads_preserve_stereo_s16le_frame_alignment() {
        struct ChunkedReader<'a> {
            data: &'a [u8],
            sizes: &'a [usize],
            step: usize,
        }
        impl Read for ChunkedReader<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.data.is_empty() {
                    return Ok(0);
                }
                let want = self
                    .sizes
                    .get(self.step)
                    .copied()
                    .unwrap_or(self.data.len());
                self.step += 1;
                let n = want.min(self.data.len()).min(out.len());
                out[..n].copy_from_slice(&self.data[..n]);
                self.data = &self.data[n..];
                Ok(n)
            }
        }

        let samples: [i16; 8] = [1000, -1000, 2000, -2000, 3000, -3000, 4000, -4000];
        let raw_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

        let buffer = Arc::new(RwLock::new(RollingAudioBuffer::new(1, 48000, 2)));
        let mut reader = ChunkedReader {
            data: &raw_bytes,
            sizes: &[1, 2, 3, 1, 5, 2, 2],
            step: 0,
        };
        let mut buf = [0u8; 4096];
        let mut carry = 0usize;
        while read_capture_chunk(&mut reader, &mut buf, &mut carry, &buffer).unwrap() > 0 {}
        assert_eq!(carry, 0);
        assert_eq!(buffer.read().unwrap().snapshot_samples(), samples);
    }
}
