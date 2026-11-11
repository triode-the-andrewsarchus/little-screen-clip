//! Cleanup ownership before an extracted slice is exposed to another process.
use std::{
    fs::File,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::PathBuf,
};

pub(in crate::audio_preview) struct AudioSliceArtifact {
    pub(in crate::audio_preview) path: PathBuf,
    directory: File,
    installed: File,
    retained: bool,
}
impl AudioSliceArtifact {
    pub(in crate::audio_preview) fn new(path: PathBuf, directory: File, installed: File) -> Self {
        Self {
            path,
            directory,
            installed,
            retained: false,
        }
    }
    pub(in crate::audio_preview) fn preserve(&mut self) {
        self.retained = true;
    }
    pub(in crate::audio_preview) fn retain(mut self) -> PathBuf {
        self.preserve();
        self.path.clone()
    }
}
impl Drop for AudioSliceArtifact {
    fn drop(&mut self) {
        if self.retained {
            return;
        }
        let Some(name) = self.path.file_name() else {
            return;
        };
        let anchor = PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()));
        let target = anchor.join(name);
        let same_file = self.installed.metadata().and_then(|held| {
            let named = std::fs::symlink_metadata(&target)?;
            Ok(named.is_file() && (held.dev(), held.ino()) == (named.dev(), named.ino()))
        });
        if same_file.unwrap_or(false) {
            let _ = std::fs::remove_file(target);
        }
    }
}
