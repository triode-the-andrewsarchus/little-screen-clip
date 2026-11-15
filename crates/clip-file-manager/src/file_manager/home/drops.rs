//! Copy-only Home drops classify on a worker before offering folder placement.
use super::*;
use little_clip_core::{file_source::ReadCheck, kernel_interface};
use std::{collections::HashMap, os::fd::AsRawFd};

pub(in crate::file_manager) enum Item {
    Artwork(PathBuf),
    Folder(PathBuf, bool),
}
#[derive(Default)]
pub(in crate::file_manager) struct Drops {
    reading: Option<Task<Result<Vec<Item>, String>>>,
    pub items: Vec<Item>,
    pub point: [f64; 2],
    token: Option<u64>,
    pub project: Option<PathBuf>,
    cancelled: bool,
}
impl Drops {
    fn active(&self) -> bool {
        self.reading.is_some()
            || !self.items.is_empty()
            || self.project.is_some()
            || self.token.is_some()
    }
}
impl Home {
    pub fn receive_drop(
        &mut self,
        paths: Vec<PathBuf>,
        point: [f64; 2],
        token: u64,
        guards: HashMap<PathBuf, super::super::search::SourceGuard>,
    ) -> bool {
        if !self.open
            || !self.ready()
            || self.drops.active()
            || !(1..=32).contains(&paths.len())
            || point.iter().any(|v| !v.is_finite() || v.abs() > 1e8)
        {
            return false;
        }
        self.drops = Drops {
            point,
            token: Some(token),
            reading: Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(5),
                move |cancel| {
                    let mut items = Vec::new();
                    for path in paths {
                        if let Some(guard) = guards.get(&path) {
                            guard.check(cancel)?;
                        }
                        if let Ok(directory) = kernel_interface::pin_content_directory(&path) {
                            let held =
                                PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
                            let manifest =
                                std::fs::symlink_metadata(held.join(".fmproject")).is_ok();
                            let folder =
                                std::fs::read_link(&held).map_err(|error| error.to_string())?;
                            items.push(Item::Folder(folder, manifest));
                        } else if artwork::supported(&path) {
                            let (_file, source) = ReadCheck::default().open(&path, None, cancel)?;
                            source.check()?;
                            items.push(Item::Artwork(path));
                        } else {
                            return Err(
                                "Drop a folder, SVG, .excalidraw or .note artwork onto Home".into(),
                            );
                        }
                    }
                    items.reverse();
                    Ok(items)
                },
            )),
            ..Default::default()
        };
        true
    }
    pub(super) fn prepare_drop_close(&mut self) -> bool {
        if self.drops.reading.is_some()
            || (self.drops.token.is_some()
                && self.drops.items.is_empty()
                && self.drops.project.is_none())
            || (self.drops.project.is_some()
                && self.projects.form.is_none()
                && self.projects.error.is_none())
        {
            return false;
        }
        self.cancel_drop();
        true
    }
    pub fn cancel_drop(&mut self) {
        if let Some(token) = self.drops.token.take() {
            self.native_results.push((token, false));
        }
        self.drops = Drops::default();
    }
    pub(super) fn tick_drops(&mut self) {
        if let Some(result) = self.drops.reading.as_ref().and_then(Task::poll) {
            self.drops.reading = None;
            match result
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            {
                Ok(items) => self.drops.items = items,
                Err(error) => {
                    self.error = Some(error);
                    self.cancel_drop();
                    return;
                }
            }
        }
        if !self.ready() {
            return;
        }
        if let Some(folder) = self.drops.project.clone()
            && self.projects.ready()
        {
            if let Some(record) = self
                .projects
                .records()
                .iter()
                .find(|record| record.folder == folder)
            {
                let project = record.id;
                let title = record.title.clone();
                let [x, y] = self.drops.point;
                self.session.edit(|board| {
                    board.cards.push(Card {
                        id: Uuid::new_v4(),
                        rect: [x - 260., y - 140., 520., 280.],
                        content: Content::Project { project, title },
                    })
                });
                self.drops.project = None;
            } else if self.projects.form.is_none() && self.projects.conflict.is_none() {
                self.drops.project = None;
                self.drops.cancelled = true;
            }
        }
        if let Some(Item::Artwork(_)) = self.drops.items.last() {
            if let Some(Item::Artwork(path)) = self.drops.items.pop() {
                self.import_artwork(vec![path], self.drops.point, None);
            }
        } else if self.drops.reading.is_none()
            && self.drops.items.is_empty()
            && self.drops.project.is_none()
            && let Some(token) = self.drops.token.take()
        {
            self.native_results.push((token, !self.drops.cancelled));
        }
    }
}
