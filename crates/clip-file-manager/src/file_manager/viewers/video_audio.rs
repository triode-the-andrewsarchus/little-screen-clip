//! A guarded stereo snapshot for editing video sound without modifying the video.
use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::{media_backend, staging::ArtifactStore};
use std::{
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

pub(super) struct Prepared {
    pub source: FileSource,
    metadata: Option<little_clip_core::audio::AudioMetadata>,
    save_directory: PathBuf,
}
impl Prepared {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (input, observed) = ReadCheck::default().open(path, None, cancel)?;
        let workspace = ArtifactStore::new()
            .map_err(|e| e.to_string())?
            .create_temporary_workspace("video-audio", "stereo")
            .map_err(|e| e.to_string())?;
        let directory = little_clip_core::kernel_interface::pin_content_directory(workspace.path())
            .map_err(|e| e.to_string())?;
        let name = format!(
            "{}.audio.wav",
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .chars()
                .take(60)
                .collect::<String>()
        );
        let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let output = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(anchor.join(name))
            .map_err(|e| e.to_string())?;
        let mut settings = media_backend::AudioRender::new(
            vec![media_backend::AudioInput {
                formats: media_backend::VIDEO_FORMATS.into(),
                stream: None,
            }],
            "wav",
        );
        settings.codec = Some("pcm_f32le".into());
        settings.channels = Some(2);
        let result = media_backend::render_audio_to_request(&settings, &output)?
            .arg_owned_content_file(input.try_clone().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
            .timeout(Duration::from_secs(120))
            .run_cancellable(cancel)
            .map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(format!(
                "Unable to extract audio: {}",
                result.stderr_lossy()
            ));
        }
        observed.check_file(&input)?;
        // Capturing this held output also retains its registered workspace for
        // deferred studio readers and accepted exports after the viewer closes.
        let source = FileSource::capture_file(&output)?;
        let metadata = little_clip_core::audio::read_audio_metadata(source.path());
        source.check_file(&output)?;
        Ok(Self {
            source,
            metadata,
            save_directory: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        })
    }
    pub fn studio(
        self,
        config: &little_clip_core::AppConfig,
    ) -> crate::audio_preview::AudioPreviewState {
        let path = self.source.path().to_path_buf();
        let held = self.source.clone();
        let check = ReadCheck::new(move |_| held.check());
        let mut studio = crate::audio_preview::AudioPreviewState::new_with_metadata_checked(
            path.clone(),
            super::super::display_path(&path),
            self.metadata,
            check,
        )
        .with_editor(config.audio.editor.clone())
        .with_file_source(self.source);
        studio.save_destination_dir = self.save_directory.to_string_lossy().into_owned();
        studio
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_audio_is_an_owned_stereo_snapshot_and_never_replaces_the_video() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/video/playback-tracks.mkv");
        let original = std::fs::read(&path).unwrap();
        let prepared = Prepared::load(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(prepared.metadata.as_ref().unwrap().channels, 2);
        let extracted = prepared.source.path().to_path_buf();
        assert_ne!(extracted, path);
        let mut wav = hound::WavReader::open(&extracted).unwrap();
        assert_eq!(wav.spec().channels, 2);
        assert!(
            wav.samples::<f32>()
                .take(48000)
                .map(|s| s.unwrap().abs())
                .sum::<f32>()
                > 10.0
        );
        drop(wav);
        let studio = prepared.studio(&little_clip_core::AppConfig::default());
        assert!(
            extracted.exists(),
            "Audio Studio must own its extracted backing file"
        );
        assert_eq!(
            studio.save_destination_dir,
            path.parent().unwrap().to_string_lossy()
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        drop(studio);
    }
    #[test]
    fn extraction_honors_cancellation_and_reports_video_without_audio() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../little-clip-core/tests/fixtures/video");
        assert!(Prepared::load(&root.join("playback-tracks.mkv"), &AtomicBool::new(true)).is_err());
        assert!(Prepared::load(&root.join("with-cover.mp4"), &AtomicBool::new(false)).is_err());
    }
}
