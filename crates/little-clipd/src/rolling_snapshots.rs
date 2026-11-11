//! Bounded publication of temporary rolling WAVs and reclamation of inactive snapshots.
use little_clip_core::rolling_audio::RollingAudioBuffer;
use std::{
    fs::{self, Metadata},
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime},
};

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 256;
const HANDOFF_GRACE: Duration = Duration::from_secs(10);
static PUBLISH: Mutex<()> = Mutex::new(());

pub(super) fn publish(root: &Path, audio: &RollingAudioBuffer) -> io::Result<(f32, PathBuf)> {
    let _publisher = PUBLISH.lock().unwrap_or_else(|error| error.into_inner());
    let directory = little_clip_core::kernel_interface::create_content_directory(root)?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let encoded_bytes = 44 + audio.len_samples() as u64 * 2;
    let budget = MAX_BYTES.checked_sub(encoded_bytes).ok_or_else(|| {
        io::Error::other("Rolling audio snapshot exceeds its temporary storage budget")
    })?;
    make_room(&anchor, budget, SystemTime::now())?;

    let mut staged = tempfile::NamedTempFile::new_in(&anchor)?;
    let duration = audio.write_wav_to(staged.as_file_mut())?;
    let name = format!("snapshot-{}.wav", uuid::Uuid::new_v4());
    let file = staged.persist_noclobber(anchor.join(name))?;
    let path = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    Ok((duration, path))
}

fn make_room(directory: &Path, budget: u64, now: SystemTime) -> io::Result<()> {
    let mut candidates = Vec::new();
    let mut total = 0u64;
    let mut count = 0usize;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let id = name
            .to_str()
            .and_then(|name| name.strip_prefix("snapshot-"))
            .and_then(|name| name.strip_suffix(".wav"));
        if !id.is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified()?;
        total = total.saturating_add(metadata.len());
        count += 1;
        if now
            .duration_since(modified)
            .is_ok_and(|age| age >= HANDOFF_GRACE)
        {
            candidates.push((modified, entry.path(), metadata));
        }
    }
    candidates.sort_unstable_by_key(|(modified, _, _)| *modified);
    for (_, path, metadata) in candidates {
        if total <= budget && count < MAX_SNAPSHOTS {
            break;
        }
        if reclaim(&path, &metadata)? {
            total = total.saturating_sub(metadata.len());
            count -= 1;
        }
    }
    if total > budget || count >= MAX_SNAPSHOTS {
        return Err(io::Error::other(
            "Rolling audio snapshot storage is busy. Retry after active snapshots are released.",
        ));
    }
    Ok(())
}

fn reclaim(path: &Path, observed: &Metadata) -> io::Result<bool> {
    let file = match little_clip_core::kernel_interface::open_content(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(_) => return Ok(false),
    };
    if file.try_lock().is_err() {
        return Ok(false);
    }
    let same = |meta: &Metadata| {
        meta.is_file()
            && (
                meta.dev(),
                meta.ino(),
                meta.len(),
                meta.mtime(),
                meta.mtime_nsec(),
            ) == (
                observed.dev(),
                observed.ino(),
                observed.len(),
                observed.mtime(),
                observed.mtime_nsec(),
            )
    };
    if !same(&file.metadata()?) || !same(&fs::symlink_metadata(path)?) {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_skips_recent_and_shared_locked_snapshots_until_released() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = || {
            let path = directory
                .path()
                .join(format!("snapshot-{}.wav", uuid::Uuid::new_v4()));
            fs::write(&path, b"audio").unwrap();
            path
        };
        let leased = fixture();
        let retired = fixture();
        let recent = fixture();
        for path in [&leased, &retired] {
            fs::File::open(path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH)
                .unwrap();
        }
        let lease = fs::File::open(&leased).unwrap();
        lease.try_lock_shared().unwrap();

        let held =
            little_clip_core::kernel_interface::pin_content_directory(directory.path()).unwrap();
        let anchor = PathBuf::from(format!("/proc/self/fd/{}", held.as_raw_fd()));
        let now = SystemTime::now();
        assert!(make_room(&anchor, 5, now).is_err());
        assert!(leased.exists());
        assert!(recent.exists());
        assert!(!retired.exists());

        drop(lease);
        make_room(&anchor, 5, now).unwrap();
        assert!(!leased.exists());
        assert!(recent.exists());
    }

    #[test]
    fn publication_exposes_a_complete_wav_with_a_private_name() {
        let directory = tempfile::tempdir().unwrap();
        let mut audio = RollingAudioBuffer::new(1, 8000, 1);
        audio.push_samples(&[1, 2, 3]);
        let (duration, path) = publish(directory.path(), &audio).unwrap();
        assert_eq!(duration, 3.0 / 8000.0);
        assert_eq!(path.parent(), Some(directory.path()));
        let bytes = fs::read(path).unwrap();
        assert_eq!(bytes.len(), 50);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[44..], &[1, 0, 2, 0, 3, 0]);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
