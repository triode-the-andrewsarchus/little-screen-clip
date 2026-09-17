//! Owned temporary artifacts with explicit retention and cleanup semantics.
//!
//! All application generated files live below the XDG cache directory.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

/// Error returned while creating, writing, or reclaiming temporary artifacts.
#[derive(Debug, Error)]
pub enum StagingError {
    /// An operating-system file operation failed.
    #[error("artifact I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Owns the application cache location used for temporary artifacts.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    base_dir: PathBuf,
}

/// Owns one temporary file or directory until it is retained or dropped.
#[derive(Debug)]
pub struct ArtifactLease {
    path: PathBuf,
    retained: bool,
}

impl ArtifactStore {
    /// Opens the standard artifact store, creating it when necessary.
    pub fn new() -> Result<Self, StagingError> {
        let base_dir = dirs_or_fallback_cache()
            .join("little-screen-clip")
            .join("artifacts");
        fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Returns the directory that contains all owned artifacts.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Creates an empty, drop-cleaned workspace directory for one operation.
    pub fn create_workspace(
        &self,
        scope: &str,
        prefix: &str,
    ) -> Result<ArtifactLease, StagingError> {
        let path = self.scoped_path(scope, prefix, None);
        fs::create_dir_all(&path)?;
        Ok(ArtifactLease {
            path,
            retained: false,
        })
    }

    /// Reserves a drop-cleaned file path in a named scope.
    pub fn create_file(
        &self,
        scope: &str,
        prefix: &str,
        extension: &str,
    ) -> Result<ArtifactLease, StagingError> {
        let path = self.scoped_path(scope, prefix, Some(extension));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(ArtifactLease {
            path,
            retained: false,
        })
    }

    /// Atomically writes bytes and retains the result for path-only consumers.
    pub fn stage_bytes(
        &self,
        scope: &str,
        prefix: &str,
        extension: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, StagingError> {
        let lease = self.create_file(scope, prefix, extension)?;
        lease.write_atomic(bytes)?;
        Ok(lease.retain())
    }

    /// Removes retained artifacts older than max_age_seconds.
    pub fn cleanup_old_items(&self, max_age_seconds: u64) -> Result<usize, StagingError> {
        let now = std::time::SystemTime::now();
        let mut removed = 0;
        cleanup_directory(&self.base_dir, now, max_age_seconds, &mut removed)?;
        Ok(removed)
    }

    fn scoped_path(&self, scope: &str, prefix: &str, extension: Option<&str>) -> PathBuf {
        let scope = safe_component(scope, "general");
        let prefix = safe_component(prefix, "artifact");
        let name = match extension.map(|extension| extension.trim_start_matches('.')) {
            Some(extension) if !extension.is_empty() => format!(
                "{prefix}_{}.{}",
                Uuid::new_v4(),
                safe_component(extension, "bin")
            ),
            _ => format!("{prefix}_{}", Uuid::new_v4()),
        };
        self.base_dir.join(scope).join(name)
    }
}

impl ArtifactLease {
    /// Returns the path owned by this lease.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Atomically replaces the leased file with bytes.
    pub fn write_atomic(&self, bytes: &[u8]) -> Result<(), StagingError> {
        let temporary = self.path.with_extension(format!("tmp-{}", Uuid::new_v4()));
        let result = (|| -> Result<(), std::io::Error> {
            let mut file = File::create(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(StagingError::from)
    }

    /// Keeps this artifact after the lease is dropped and returns its path.
    pub fn retain(mut self) -> PathBuf {
        self.retained = true;
        self.path.clone()
    }
}

impl Drop for ArtifactLease {
    fn drop(&mut self) {
        if !self.retained {
            let _ = if self.path.is_dir() {
                fs::remove_dir_all(&self.path)
            } else {
                fs::remove_file(&self.path)
            };
        }
    }
}

/// Backwards-compatible name for callers that only need the artifact store.
pub type StagingManager = ArtifactStore;

/// Returns the standard artifact directory, creating it if necessary.
pub fn get_staging_dir() -> Result<PathBuf, StagingError> {
    Ok(ArtifactStore::new()?.base_dir().to_path_buf())
}

/// Reserves a legacy staging path. Prefer ArtifactStore for new code.
pub fn create_staging_path(prefix: &str, extension: &str) -> Result<PathBuf, StagingError> {
    let lease = ArtifactStore::new()?.create_file("legacy", prefix, extension)?;
    Ok(lease.retain())
}

fn cleanup_directory(
    directory: &Path,
    now: std::time::SystemTime,
    max_age_seconds: u64,
    removed: &mut usize,
) -> Result<(), StagingError> {
    for entry in fs::read_dir(directory)? {
        cleanup_entry(&entry?, now, max_age_seconds, removed)?;
    }
    Ok(())
}

fn cleanup_entry(
    entry: &fs::DirEntry,
    now: std::time::SystemTime,
    max_age_seconds: u64,
    removed: &mut usize,
) -> Result<(), StagingError> {
    let path = entry.path();
    let metadata = entry.metadata()?;
    if metadata.is_dir() {
        cleanup_directory(&path, now, max_age_seconds, removed)?;
        remove_empty_directory(&path)?;
    } else if is_expired(&metadata, now, max_age_seconds)? {
        fs::remove_file(path)?;
        *removed += 1;
    }
    Ok(())
}

fn remove_empty_directory(path: &Path) -> Result<(), StagingError> {
    if fs::read_dir(path)?.next().is_none() {
        let _ = fs::remove_dir(path);
    }
    Ok(())
}

fn is_expired(
    metadata: &fs::Metadata,
    now: std::time::SystemTime,
    max_age_seconds: u64,
) -> Result<bool, StagingError> {
    Ok(now
        .duration_since(metadata.modified()?)
        .is_ok_and(|age| age.as_secs() > max_age_seconds))
}

fn safe_component(value: &str, fallback: &str) -> String {
    let filtered: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .collect();
    if filtered.is_empty() {
        fallback.to_string()
    } else {
        filtered
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> ArtifactStore {
        let base_dir =
            std::env::temp_dir().join(format!("little-clip-artifacts-{}", Uuid::new_v4()));
        fs::create_dir_all(&base_dir).unwrap();
        ArtifactStore { base_dir }
    }

    #[test]
    fn atomic_write_is_visible_at_the_retained_path() {
        let store = test_store();
        let path = store
            .stage_bytes("clipboard", "image", "png", b"pixels")
            .unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"pixels");
        fs::remove_dir_all(store.base_dir()).unwrap();
    }

    #[test]
    fn unretained_workspace_is_removed_when_lease_drops() {
        let store = test_store();
        let path = {
            let workspace = store.create_workspace("pipeline", "run").unwrap();
            workspace.path().to_path_buf()
        };

        assert!(!path.exists());
        fs::remove_dir_all(store.base_dir()).unwrap();
    }
}
