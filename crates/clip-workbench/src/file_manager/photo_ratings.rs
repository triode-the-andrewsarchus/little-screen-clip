//! Ratings live in application data, independently of source pixels and editor history.
use egui::Context;
use little_clip_core::{BackgroundJobs, JobError};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

pub(super) const STAR_COLOR: egui::Color32 = egui::Color32::from_rgb(245, 196, 85);

pub(super) fn paint_badge(ui: &egui::Ui, rect: egui::Rect, rating: u8) {
    if rating == 0 {
        return;
    }
    ui.painter()
        .rect_filled(rect, 3.0, egui::Color32::from_black_alpha(190));
    let edge = rect.height() - 4.0;
    for star in 0..rating.min(5) {
        crate::icons::Icon::Star.paint(
            ui.painter(),
            egui::Rect::from_min_size(
                rect.min + egui::vec2(4.0 + star as f32 * (edge + 1.0), 2.0),
                egui::Vec2::splat(edge),
            ),
            STAR_COLOR,
        );
    }
}

type Response = mpsc::Receiver<Result<Result<u8, String>, JobError>>;
#[derive(Default)]
pub(super) struct Ratings {
    pub values: BTreeMap<PathBuf, u8>,
    writes: BTreeMap<PathBuf, u8>,
    pending: Option<(PathBuf, Option<u8>, Response)>,
}
impl Ratings {
    pub fn set(&mut self, path: PathBuf, value: u8) {
        self.values.insert(path.clone(), value.min(5));
        self.writes.insert(path, value.min(5));
    }
    pub fn busy(&self) -> bool {
        !self.writes.is_empty()
            || self
                .pending
                .as_ref()
                .is_some_and(|(_, value, _)| value.is_some())
    }
    pub fn poll(&mut self, ctx: &Context, visible: &[PathBuf]) -> Option<String> {
        let (ready, error) = self.receive(ctx);
        if !ready {
            return error;
        }
        let next = self
            .writes
            .pop_first()
            .map(|(p, v)| (p, Some(v)))
            .or_else(|| {
                visible
                    .iter()
                    .find(|p| !self.values.contains_key(*p))
                    .map(|p| (p.clone(), None))
            });
        if let Some((path, value)) = next {
            let target = path.clone();
            let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
                if let Some(value) = value {
                    save(&target, value)?;
                    Ok(value)
                } else {
                    load(&target)
                }
            });
            self.pending = Some((path, value, receiver));
            ctx.request_repaint_after(Duration::from_millis(40));
        }
        error
    }
    fn receive(&mut self, ctx: &Context) -> (bool, Option<String>) {
        let mut error = None;
        if let Some((path, value, receiver)) = &self.pending {
            match receiver.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return (false, None);
                }
                Ok(Err(JobError::QueueFull)) => {
                    if let Some(value) = value {
                        self.writes.entry(path.clone()).or_insert(*value);
                    }
                }
                result => {
                    let result = result
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()))
                        .and_then(|r| r);
                    match result {
                        Ok(rating) => {
                            self.values.entry(path.clone()).or_insert(rating);
                        }
                        Err(message) => {
                            self.values.entry(path.clone()).or_insert(0);
                            error = Some(format!(
                                "Rating could not be {}: {message}",
                                if value.is_some() { "saved" } else { "loaded" }
                            ));
                        }
                    }
                }
            }
            self.pending = None;
        }
        (true, error)
    }
}
fn location(path: &Path) -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .ok_or("Application data directory unavailable")?;
    let hash = Sha256::digest(path.as_os_str().as_bytes());
    Ok(root
        .join("little-screen-clip/photo-ratings-v1")
        .join(format!("{hash:x}.json")))
}
pub(in crate::file_manager) fn load(path: &Path) -> Result<u8, String> {
    match crate::file_source::read_file_bytes(&location(path)?, 16, true, false) {
        Ok(bytes) => serde_json::from_slice::<u8>(&bytes)
            .map(|v| v.min(5))
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.to_string()),
    }
}
/// Follow ordinary Files renames without changing the mutation's completion result.
pub(super) fn relocate(source: &Path, destination: &Path) -> Result<(), String> {
    if source == destination {
        return Ok(());
    }
    let rating = load(source)?;
    if rating > 0 {
        save(destination, rating)?;
        save(source, 0)?;
    }
    Ok(())
}
fn save(path: &Path, value: u8) -> Result<(), String> {
    let path = location(path)?;
    let directory =
        little_clip_core::kernel_interface::create_content_directory(path.parent().unwrap())
            .map_err(|e| e.to_string())?;
    use std::os::fd::AsRawFd;
    let parent = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut temporary = tempfile::NamedTempFile::new_in(&parent).map_err(|e| e.to_string())?;
    write!(temporary, "{}", value.min(5))
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    temporary
        .persist(parent.join(path.file_name().unwrap()))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_rating_read_cannot_replace_a_new_rating() {
        let path = PathBuf::from("photo.png");
        let (sender, receiver) = mpsc::channel();
        let mut ratings = Ratings {
            pending: Some((path.clone(), None, receiver)),
            ..Default::default()
        };
        ratings.set(path.clone(), 5);
        sender.send(Ok(Ok(1))).unwrap();
        ratings.receive(&Context::default());
        assert_eq!(ratings.values.get(&path), Some(&5));
        // The pending write remains part of close protection.
        assert!(ratings.busy());
    }
    #[test]
    fn newer_rating_wins_when_queue_admission_is_retried() {
        let path = PathBuf::from("photo.png");
        let (sender, receiver) = mpsc::channel();
        let mut ratings = Ratings {
            pending: Some((path.clone(), Some(1), receiver)),
            ..Default::default()
        };
        ratings.set(path.clone(), 4);
        sender.send(Err(JobError::QueueFull)).unwrap();
        ratings.receive(&Context::default());
        assert_eq!(ratings.writes.get(&path), Some(&4));
        assert_eq!(ratings.values.get(&path), Some(&4));
    }
}
