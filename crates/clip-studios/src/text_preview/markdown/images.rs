//! Bounded, off-UI-thread local and Hugo-`static` image decoding cache for Markdown.

use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const MAX_CACHED_IMAGES: usize = 192;
const MARKDOWN_IMAGE_EDGE: u32 = 1280;
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const RETRY_DELAY: Duration = Duration::from_millis(250);

struct LoadedImage {
    color_image: egui::ColorImage,
    pixel_size: [u32; 2],
}

enum CacheEntry {
    Loading {
        task: Task<Option<LoadedImage>>,
    },
    RetryAt(Instant),
    Ready {
        texture: egui::TextureHandle,
        pixel_size: [u32; 2],
    },
    Unavailable,
}

pub(in crate::text_preview) enum ImageStatus {
    Loading,
    Ready {
        texture: egui::TextureHandle,
        pixel_size: [u32; 2],
    },
    Unavailable,
}

#[derive(Default)]
pub(crate) struct ImageCache {
    base_doc: Option<PathBuf>,
    entries: HashMap<String, CacheEntry>,
}

impl ImageCache {
    pub(super) fn set_base_path(&mut self, path: Option<&Path>) {
        if self.base_doc.as_deref() != path {
            self.base_doc = path.map(Path::to_path_buf);
            self.entries.clear();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(super) fn resolve(&mut self, ui: &egui::Ui, reference: &str) -> ImageStatus {
        let Some(doc_path) = self.base_doc.clone() else {
            return ImageStatus::Unavailable;
        };

        if let Some(entry) = self.entries.get_mut(reference) {
            match entry {
                CacheEntry::Loading { task } => {
                    if let Some(delivery) = task.poll() {
                        match delivery {
                            Ok(Some(loaded)) => {
                                let texture = ui.ctx().load_texture(
                                    format!("md_img:{reference}"),
                                    loaded.color_image,
                                    egui::TextureOptions::LINEAR,
                                );
                                let pixel_size = loaded.pixel_size;
                                *entry = CacheEntry::Ready {
                                    texture: texture.clone(),
                                    pixel_size,
                                };
                                ui.ctx().request_repaint();
                                return ImageStatus::Ready {
                                    texture,
                                    pixel_size,
                                };
                            }
                            Ok(None) => {
                                *entry = CacheEntry::Unavailable;
                                return ImageStatus::Unavailable;
                            }
                            Err(JobError::QueueFull) => {
                                *entry = CacheEntry::RetryAt(Instant::now() + RETRY_DELAY);
                                ui.ctx().request_repaint_after(RETRY_DELAY);
                                return ImageStatus::Loading;
                            }
                            Err(_) => {
                                *entry = CacheEntry::Unavailable;
                                return ImageStatus::Unavailable;
                            }
                        }
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(24));
                    return ImageStatus::Loading;
                }
                CacheEntry::RetryAt(due) => {
                    if Instant::now() < *due {
                        ui.ctx().request_repaint_after(Duration::from_millis(40));
                        return ImageStatus::Loading;
                    }
                    let task = spawn_image_load(doc_path, reference.to_owned());
                    *entry = CacheEntry::Loading { task };
                    ui.ctx().request_repaint_after(Duration::from_millis(24));
                    return ImageStatus::Loading;
                }
                CacheEntry::Ready {
                    texture,
                    pixel_size,
                } => {
                    return ImageStatus::Ready {
                        texture: texture.clone(),
                        pixel_size: *pixel_size,
                    };
                }
                CacheEntry::Unavailable => return ImageStatus::Unavailable,
            }
        }

        if is_obviously_external(reference) || self.entries.len() >= MAX_CACHED_IMAGES {
            if is_obviously_external(reference) && self.entries.len() < MAX_CACHED_IMAGES {
                self.entries
                    .insert(reference.to_owned(), CacheEntry::Unavailable);
            }
            return ImageStatus::Unavailable;
        }

        let task = spawn_image_load(doc_path, reference.to_owned());
        self.entries
            .insert(reference.to_owned(), CacheEntry::Loading { task });
        ui.ctx().request_repaint_after(Duration::from_millis(24));
        ImageStatus::Loading
    }
}

fn spawn_image_load(doc_path: PathBuf, reference: String) -> Task<Option<LoadedImage>> {
    Task::start(BackgroundJobs::global(), LOAD_TIMEOUT, move |cancel| {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let resolved =
            little_clip_core::markdown_assets::resolve_document_asset(&doc_path, &reference)?;
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let thumb = little_clip_core::gallery_image::thumbnail::load(
            &resolved,
            MARKDOWN_IMAGE_EDGE,
            1,
            cancel,
        )?;
        let pixel_size = [thumb.width.max(1), thumb.height.max(1)];
        let color_image = egui::ColorImage::from_rgba_premultiplied(
            [thumb.width as usize, thumb.height as usize],
            &thumb.premultiplied_rgba,
        );
        Some(LoadedImage {
            color_image,
            pixel_size,
        })
    })
}

fn is_obviously_external(reference: &str) -> bool {
    let trimmed = reference.trim().to_ascii_lowercase();
    trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("data:")
        || trimmed.starts_with("file:")
        || trimmed.starts_with("javascript:")
        || trimmed.starts_with("vbscript:")
        || trimmed.starts_with("blob:")
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with("//")
}
