//! Owned temporary selections for studio intake, using the shared audio renderer.
use super::*;
use crate::file_source::{FileSource, ReadCheck};
use std::os::fd::AsRawFd;

/// Owns a temporary audio selection until its viewer and deferred readers retire.
///
/// Dropping an unpublished result cleans its file through a retained directory
/// descriptor. It is not a path-only export for external clipboard consumers.
pub struct PreparedAudioSlice {
    path: PathBuf,
    source: FileSource,
    check: ReadCheck,
    metadata: Option<AudioMetadata>,
}

impl PreparedAudioSlice {
    /// Returns the selection's path while this result or its resulting viewer owns it.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Transfers temporary-file ownership into an independent Audio Studio session.
    ///
    /// Metadata is already prepared; analysis and later reads run on session workers.
    pub fn into_preview(self, title: String) -> AudioPreviewState {
        AudioPreviewState::new_with_metadata_checked(self.path, title, self.metadata, self.check)
            .with_file_source(self.source)
    }
}

/// Prepares a checked, range-bounded temporary WAV for an internal studio.
///
/// Runs decoding and metadata work on the calling worker. Optional edge-silence
/// removal falls back to the already selected audio if that filter fails.
/// Cancellation and changed sources fail rather than publishing the fallback.
///
/// # Errors
/// Returns an error for invalid ranges, changed/protected sources, cancellation,
/// unavailable decoding, or failure to stage an owned output.
pub fn prepare_audio_slice(
    source: &FileSource,
    start: f32,
    end: f32,
    trim_silence: bool,
    cancelled: &AtomicBool,
) -> Result<PreparedAudioSlice, String> {
    prepare_with_filter(source, start, end, trim_silence.then_some(
        "silenceremove=start_periods=1:start_duration=0.05:start_threshold=-45dB,areverse,silenceremove=start_periods=1:start_duration=0.05:start_threshold=-45dB,areverse"
    ), cancelled)
}

fn prepare_with_filter(
    source: &FileSource,
    start: f32,
    end: f32,
    trim_filter: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<PreparedAudioSlice, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Audio selection cancelled".into());
    }
    if !start.is_finite() || !end.is_finite() || start.min(end) < 0.0 || (end - start).abs() < 0.001
    {
        return Err("Select a non-empty audio range first.".into());
    }
    let region = AudioSaveRegion {
        start: start.min(end),
        end: start.max(end),
        filename: "selection.wav".into(),
        filter: None,
    };
    let selected = session_edit::render_audio_file(
        source.path(),
        Some(source),
        &ReadCheck::default(),
        &region,
        export::AudioRenderMode::Export,
        cancelled,
        None,
    )?;
    let rendered = if let Some(filter) = trim_filter {
        trim_selection(&selected, region.end - region.start, filter, cancelled).unwrap_or(selected)
    } else {
        selected
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("Audio selection cancelled".into());
    }
    let descriptor = PathBuf::from(format!(
        "/proc/self/fd/{}",
        rendered.staging.as_file().as_raw_fd()
    ));
    let identity = FileSource::capture_file(rendered.staging.as_file())?;
    let metadata = read_audio_metadata(&descriptor);
    source.check()?;
    let path = identity.path().to_owned();
    let check =
        session_edit::retain_session_file(rendered.staging, rendered.directory, identity.clone());
    check.validate(cancelled)?;
    Ok(PreparedAudioSlice {
        path,
        source: identity,
        check,
        metadata,
    })
}

fn trim_selection(
    selected: &session_edit::RenderedSessionFile,
    duration: f32,
    filter: &str,
    cancelled: &AtomicBool,
) -> Result<session_edit::RenderedSessionFile, String> {
    let path = PathBuf::from(format!(
        "/proc/self/fd/{}",
        selected.staging.as_file().as_raw_fd()
    ));
    let identity = FileSource::capture_file(selected.staging.as_file())?;
    let region = AudioSaveRegion {
        start: 0.0,
        end: duration,
        filename: "trimmed.wav".into(),
        filter: Some(filter.into()),
    };
    session_edit::render_audio_file(
        &path,
        Some(&identity),
        &ReadCheck::default(),
        &region,
        export::AudioRenderMode::Export,
        cancelled,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, FileSource, Vec<i16>) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.wav");
        let samples: Vec<i16> = (0..8000)
            .flat_map(|frame| {
                if (1600..6400).contains(&frame) {
                    [10_000, 5000]
                } else {
                    [0, 0]
                }
            })
            .collect();
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 8000, 2);
        buffer.push_samples(&samples);
        buffer.write_wav(&path).unwrap();
        let source = FileSource::capture(&path).unwrap();
        (directory, source, samples)
    }

    #[test]
    fn owned_slice_keeps_selected_channels_and_cleans_when_unpublished() {
        let (_directory, source, samples) = fixture();
        let prepared =
            prepare_audio_slice(&source, 0.8, 0.2, false, &AtomicBool::new(false)).unwrap();
        let path = prepared.path().to_owned();
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 8000);
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            samples[3200..12800]
        );
        drop(reader);
        drop(prepared);
        assert!(!path.exists());
    }

    #[test]
    fn silence_trim_and_failed_trim_keep_the_selected_range_bounded() {
        let (_directory, source, samples) = fixture();
        let trimmed =
            prepare_audio_slice(&source, 0.0, 1.0, true, &AtomicBool::new(false)).unwrap();
        // The existing filter confirms 50 ms of sound at each edge.
        assert!(
            (450..650).contains(&trimmed.metadata.as_ref().unwrap().duration_ms),
            "trimmed duration: {:?}",
            trimmed.metadata.as_ref().unwrap().duration_ms
        );
        let fallback = prepare_with_filter(
            &source,
            0.1,
            0.9,
            Some("missing_audio_filter_for_test"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut reader = hound::WavReader::open(fallback.path()).unwrap();
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            samples[1600..14400]
        );
    }

    #[test]
    fn view_readers_retain_temporary_source_until_the_last_owner_retires() {
        let (_directory, source, _) = fixture();
        let prepared =
            prepare_audio_slice(&source, 0.0, 1.0, false, &AtomicBool::new(false)).unwrap();
        let path = prepared.path().to_owned();
        let check = prepared.check.clone();
        let state = prepared.into_preview("Rolling Audio Snapshot".into());
        assert_eq!(state.path, path);
        assert!(state.file_source().is_some());
        drop(state);
        check.validate(&AtomicBool::new(false)).unwrap();
        assert!(path.is_file());
        drop(check);
        let deadline = Instant::now() + Duration::from_secs(5);
        while path.exists() {
            assert!(
                Instant::now() < deadline,
                "cancelled analysis retained the temporary source"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn temporary_cleanup_uses_the_retained_parent_after_directory_replacement() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let directory =
            little_clip_core::kernel_interface::pin_content_directory(&original).unwrap();
        let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let staging = tempfile::NamedTempFile::new_in(anchor).unwrap();
        let identity = FileSource::capture_file(staging.as_file()).unwrap();
        let name = identity.path().file_name().unwrap().to_owned();
        let check = session_edit::retain_session_file(staging, directory, identity);
        let moved = root.path().join("moved");
        std::fs::rename(&original, &moved).unwrap();
        std::fs::create_dir(&original).unwrap();
        std::fs::write(original.join(&name), b"unrelated replacement").unwrap();
        drop(check);
        assert!(!moved.join(&name).exists());
        assert_eq!(
            std::fs::read(original.join(name)).unwrap(),
            b"unrelated replacement"
        );
    }

    #[test]
    fn stale_sources_invalid_ranges_and_cancelled_reads_do_not_publish() {
        let (_directory, source, _) = fixture();
        for (start, end) in [
            (f32::NAN, 1.0),
            (0.0, f32::INFINITY),
            (-1.0, 0.5),
            (0.0, 0.0),
        ] {
            assert!(
                prepare_audio_slice(&source, start, end, false, &AtomicBool::new(false)).is_err()
            );
        }
        assert!(
            prepare_audio_slice(&source, 0.0, 1.0, false, &AtomicBool::new(true))
                .err()
                .unwrap()
                .contains("cancelled")
        );
        std::fs::write(source.path(), b"replaced snapshot").unwrap();
        assert!(prepare_audio_slice(&source, 0.0, 1.0, true, &AtomicBool::new(false)).is_err());
    }
}
