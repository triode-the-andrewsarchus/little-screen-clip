//! Temporary file staging directory management and automatic cache cleanup.
//!
//! Provides unique scratch paths for generated previews, intermediate transforms, and exports.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Error type encountered during staging directory operations.
#[derive(Debug, Error)]
pub enum StagingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Manages temporary files created during conversions, transforms, and clip previews.
pub struct StagingManager {
    base_dir: PathBuf,
}

impl StagingManager {
    /// Initializes the staging directory under `$XDG_CACHE_HOME/little-screen-clip/staging`.
    pub fn new() -> Result<Self, StagingError> {
        let cache_dir = dirs_or_fallback_cache();
        let staging_dir = cache_dir.join("little-screen-clip").join("staging");
        std::fs::create_dir_all(&staging_dir)?;

        Ok(Self {
            base_dir: staging_dir,
        })
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Creates a staging path for a new transformed item.
    pub fn create_staging_path(&self, prefix: &str, extension: &str) -> PathBuf {
        let id = uuid::Uuid::new_v4();
        let filename = if extension.is_empty() {
            format!("{}_{}", prefix, id)
        } else {
            format!("{}_{}.{}", prefix, id, extension.trim_start_matches('.'))
        };
        self.base_dir.join(filename)
    }

    /// Cleans up old staging items older than max_age_seconds.
    pub fn cleanup_old_items(&self, max_age_seconds: u64) -> Result<usize, StagingError> {
        let now = std::time::SystemTime::now();
        let mut count = 0;

        if let Ok(entries) = std::fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(age) = now.duration_since(modified) {
                            if age.as_secs() > max_age_seconds {
                                let _ = std::fs::remove_file(entry.path());
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(count)
    }
}

/// Returns the standard staging directory, creating it if necessary.
pub fn get_staging_dir() -> Result<PathBuf, StagingError> {
    let cache_dir = dirs_or_fallback_cache();
    let staging_dir = cache_dir.join("little-screen-clip").join("staging");
    std::fs::create_dir_all(&staging_dir)?;
    Ok(staging_dir)
}

/// Creates a unique staged file path in the standard staging directory.
pub fn create_staging_path(prefix: &str, extension: &str) -> Result<PathBuf, StagingError> {
    let base_dir = get_staging_dir()?;
    let id = uuid::Uuid::new_v4();
    let filename = if extension.is_empty() {
        format!("{}_{}", prefix, id)
    } else {
        format!("{}_{}.{}", prefix, id, extension.trim_start_matches('.'))
    };
    Ok(base_dir.join(filename))
}

fn dirs_or_fallback_cache() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache");
    }
    PathBuf::from("/tmp")
}
