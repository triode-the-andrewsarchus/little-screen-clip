//! Small, bounded previews for visible Shelf cards; file reads stay on workers.

use super::super::content::Payload;
use super::super::display_path;
use crate::icons::Icon;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::{
    BackgroundJobs, ClipItem, ClipPayload, ImageThumbnail, JobError, JobTicket, SlotId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Clone)]
pub(in crate::file_manager) enum Source {
    Payload(Payload),
    Slot(SlotId, uuid::Uuid),
    Gallery {
        path: PathBuf,
        edge: u32,
        orientation: u8,
    },
    Music(PathBuf),
}

pub(in crate::file_manager) use super::super::media::PreviewData as Data;

pub(in crate::file_manager) struct Ready {
    pub data: Data,
    pub texture: Option<egui::TextureHandle>,
}
struct Pending {
    ticket: JobTicket,
    receiver: mpsc::Receiver<Result<Data, JobError>>,
    cancelled: Arc<AtomicBool>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}

#[derive(Default)]
pub(in crate::file_manager) struct Previews {
    ready: BTreeMap<uuid::Uuid, Ready>,
    pending: BTreeMap<uuid::Uuid, Pending>,
}

impl Previews {
    pub fn needs(&self, id: uuid::Uuid) -> bool {
        !self.pending.contains_key(&id) && !self.ready.contains_key(&id)
    }
    pub fn clear(&mut self) {
        self.ready.clear();
        self.pending.clear();
    }
    pub fn invalidate(&mut self, id: uuid::Uuid) {
        self.ready.remove(&id);
        self.pending.remove(&id);
    }

    pub fn retain(&mut self, ids: &BTreeSet<uuid::Uuid>) {
        self.ready.retain(|id, _| ids.contains(id));
        self.pending.retain(|id, _| ids.contains(id));
    }

    pub fn get(&self, id: uuid::Uuid) -> Option<&Ready> {
        self.ready.get(&id)
    }

    pub fn request(&mut self, id: uuid::Uuid, source: Source) {
        let music = matches!(&source, Source::Music(_));
        let gallery = matches!(&source, Source::Gallery { .. });
        let executor = if gallery {
            gallery_preview_jobs()
        } else if music {
            music_preview_jobs()
        } else {
            BackgroundJobs::global()
        };
        let limit = if gallery {
            4
        } else if music {
            music_preview_workers()
        } else {
            1
        };
        if !self.needs(id) || self.pending.len() >= limit {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = cancelled.clone();
        let deadline = if matches!(&source, Source::Music(_)) {
            30
        } else {
            15
        };
        let (ticket, receiver) =
            executor.submit(Duration::from_secs(deadline), move || match source {
                Source::Payload(payload) => payload_preview(&payload),
                Source::Slot(slot, expected) => slot_preview(slot, expected),
                Source::Gallery {
                    path,
                    edge,
                    orientation,
                } => gallery_preview(&path, edge, orientation, &token),
                Source::Music(path) => super::super::music::preview::load(&path, &token),
            });
        self.pending.insert(
            id,
            Pending {
                ticket,
                receiver,
                cancelled,
            },
        );
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut completed = Vec::new();
        self.pending.retain(|id, pending| {
            let result = match pending.receiver.try_recv() {
                Ok(Err(JobError::QueueFull)) => {
                    ctx.request_repaint();
                    return false;
                }
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(_) => Err(JobError::Cancelled),
            };
            completed.push((*id, result));
            false
        });
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        for (id, result) in completed {
            self.complete(ctx, id, result);
        }
    }

    fn complete(&mut self, ctx: &egui::Context, id: uuid::Uuid, result: Result<Data, JobError>) {
        let data = result.unwrap_or_else(|error| {
            let mut data = Data::new(
                "Preview unavailable".into(),
                "Retry with Refresh previews".into(),
                Icon::Alert,
            );
            data.text = error.to_string();
            data
        });
        let texture = data.image.as_ref().map(|image| {
            ctx.load_texture(
                format!("shelf_preview_{id}"),
                egui::ColorImage::from_rgba_premultiplied(
                    [image.width as usize, image.height as usize],
                    &image.premultiplied_rgba,
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        self.ready.insert(id, Ready { data, texture });
        ctx.request_repaint();
    }
}

fn gallery_preview_jobs() -> &'static BackgroundJobs {
    static JOBS: std::sync::LazyLock<BackgroundJobs> =
        std::sync::LazyLock::new(|| BackgroundJobs::new(4, 8));
    &JOBS
}

// Music analysis has its own bounded executor so a folder cannot occupy the
// workers needed for clipboard hydration, viewers, and ordinary file operations.
fn music_preview_workers() -> usize {
    static WORKERS: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
    });
    *WORKERS
}

fn music_preview_jobs() -> &'static BackgroundJobs {
    static JOBS: std::sync::LazyLock<BackgroundJobs> =
        std::sync::LazyLock::new(|| BackgroundJobs::new(music_preview_workers(), 8));
    &JOBS
}

fn slot_preview(slot: SlotId, expected: uuid::Uuid) -> Data {
    let result = crate::desktop_ipc::request(
        &IpcRequest::GetSlotItem(slot),
        Some(Duration::from_secs(2)),
        16 * 1024 * 1024,
        1024 * 1024,
    );
    match result {
        Ok(IpcResponse::SlotItem(Some(item))) if item.id == expected => item_preview(&item),
        _ => Data::new(
            "Shared preview unavailable".into(),
            "Refresh hands to retry".into(),
            Icon::Alert,
        ),
    }
}

fn item_preview(item: &ClipItem) -> Data {
    let mut data = match &item.payload {
        ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
            text_preview(text.as_bytes(), "Text")
        }
        ClipPayload::Image {
            bytes,
            path,
            mime,
            thumbnail: existing_thumbnail,
            ..
        } => {
            let mut data = Data::new(item.title.clone(), mime.clone(), Icon::Image);
            data.image = existing_thumbnail
                .as_ref()
                .filter(|image| valid_thumbnail(image))
                .cloned();
            if data.image.is_none() {
                data.image = if !bytes.is_empty() {
                    thumbnail(bytes, Some(mime))
                } else {
                    path.as_ref()
                        .and_then(|path| {
                            crate::file_source::read_file_bytes(path, 8 * 1024 * 1024, true, false)
                                .ok()
                        })
                        .and_then(|bytes| thumbnail(&bytes, Some(mime)))
                };
            }
            data
        }
        ClipPayload::AudioRef { waveform, .. } => {
            let mut data = Data::new(item.title.clone(), item.subtitle.clone(), Icon::Audio);
            if let Some(waveform) = waveform {
                let stride = waveform.ch1_peaks.len().div_ceil(96).max(1);
                data.waveform = waveform
                    .ch1_peaks
                    .chunks(stride)
                    .map(|chunk| {
                        chunk
                            .iter()
                            .copied()
                            .filter(|value| value.is_finite())
                            .fold(0.0f32, f32::max)
                            .clamp(0.0, 1.0)
                    })
                    .take(96)
                    .collect();
                if let Some(colors) = &waveform.spectral_colors {
                    data.waveform_colors = colors
                        .chunks(stride)
                        .take(96)
                        .map(|chunk| {
                            let mut sum = [0_u32; 3];
                            for rgb in chunk {
                                for i in 0..3 {
                                    sum[i] += rgb[i] as u32;
                                }
                            }
                            sum.map(|value| (value / chunk.len() as u32) as u8)
                        })
                        .collect();
                }
            }
            data
        }
        ClipPayload::Animation {
            path,
            thumbnail: image,
            ..
        } => {
            let mut data = file_preview(&[path.clone()]);
            data.icon = Icon::Film;
            data.image = image
                .as_ref()
                .filter(|image| image.width <= 256 && image.height <= 128)
                .and_then(|image| image.frames_jpeg.first())
                .and_then(|bytes| thumbnail(bytes, Some("image/jpeg")))
                .or(data.image);
            data
        }
        ClipPayload::FileBatch(paths) | ClipPayload::MultiImage { paths, .. } => {
            file_preview(paths)
        }
        ClipPayload::Directory(path) => file_preview(&[path.clone()]),
        ClipPayload::Url(url) => text_preview(url.as_str().as_bytes(), "URL"),
        ClipPayload::Color { hex, .. } => text_preview(hex.as_bytes(), "Color"),
    };
    data.title = item.title.chars().take(120).collect();
    data
}

fn valid_thumbnail(image: &ImageThumbnail) -> bool {
    image.width > 0
        && image.height > 0
        && image.width <= 256
        && image.height <= 128
        && image.premultiplied_rgba.len() == image.width as usize * image.height as usize * 4
}

fn payload_preview(payload: &Payload) -> Data {
    match payload {
        Payload::Files { paths, .. } => file_preview(paths),
        Payload::Content(content) if content.mime.starts_with("image/") => {
            let mut data = Data::new(content.default_name(), content.mime.clone(), Icon::Image);
            data.image = thumbnail(&content.bytes, Some(&content.mime));
            data
        }
        Payload::Content(content) if content.mime == "application/pdf" => {
            Data::new(content.default_name(), "PDF document".into(), Icon::Pdf)
        }
        Payload::Content(content) => text_preview(&content.bytes, &content.extension),
    }
}

fn text_preview(bytes: &[u8], kind: &str) -> Data {
    let prefix = &bytes[..bytes.len().min(2048)];
    let text: String = String::from_utf8_lossy(prefix)
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .take(240)
        .collect();
    let title = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Empty text")
        .chars()
        .take(80)
        .collect();
    let mut data = Data::new(title, format!("{kind} · {} bytes", bytes.len()), Icon::Text);
    data.text = text;
    data
}

fn file_preview(paths: &[PathBuf]) -> Data {
    let Some(path) = paths.first() else {
        return Data::new("Empty group".into(), String::new(), Icon::Folder);
    };
    let title = display_path(Path::new(path.file_name().unwrap_or_default()));
    let mut data = Data::new(
        title,
        format!(
            "{} file{}",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        ),
        Icon::Folder,
    );
    data.text = paths
        .iter()
        .take(4)
        .map(|path| display_path(Path::new(path.file_name().unwrap_or_default())))
        .collect::<Vec<_>>()
        .join("\n");
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            data.detail = "Reference unavailable".into();
            data.icon = Icon::Alert;
            return data;
        }
    };
    if metadata.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            data.text = entries
                .take(4)
                .filter_map(Result::ok)
                .map(|entry| display_path(Path::new(&entry.file_name())))
                .collect::<Vec<_>>()
                .join("\n");
        }
    } else if metadata.is_file() {
        preview_regular_file(path, &mut data);
    }
    data
}

fn preview_regular_file(path: &Path, data: &mut Data) {
    data.icon = Icon::Text;
    if little_clip_core::item::is_image_path(path) {
        data.icon = Icon::Image;
        data.image = crate::file_source::read_file_bytes(path, 8 * 1024 * 1024, true, false)
            .ok()
            .and_then(|bytes| {
                thumbnail(
                    &bytes,
                    Some(&little_clip_core::item::guess_image_mime(path)),
                )
            });
    } else if little_clip_core::audio::is_audio_path(path) {
        data.icon = Icon::Audio;
        (data.waveform, data.waveform_colors) = audio_excerpt(path);
    } else if little_clip_core::is_video_path(path) {
        data.icon = Icon::Film;
        data.detail = "Video".into();
        data.image = super::super::media::video_preview(path);
    } else if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
    {
        data.icon = Icon::Pdf;
        if let Ok(info) = little_clip_core::pdf::get_pdf_info(path) {
            data.detail = format!("{} pages", info.page_count);
            data.text = info.title.unwrap_or_default();
        }
    } else if let Ok(bytes) = crate::file_source::read_file_bytes(path, 2048, false, false)
        && !bytes.contains(&0)
        && std::str::from_utf8(&bytes).is_ok()
    {
        data.text = text_preview(&bytes, "Text").text;
        data.icon = Icon::Text;
    }
}

fn thumbnail(bytes: &[u8], mime: Option<&str>) -> Option<ImageThumbnail> {
    sized_thumbnail(bytes, mime, 128, 80)
}

fn gallery_preview(path: &Path, edge: u32, orientation: u8, cancelled: &AtomicBool) -> Data {
    let mut data = Data::new(display_path(path), "Photo".into(), Icon::Image);
    if cancelled.load(Ordering::Relaxed) {
        return data;
    }
    data.image = super::super::gallery::thumbnail::load(path, edge, orientation, cancelled);
    if data.image.is_none() {
        data.detail = "Preview unavailable; open in Image Studio or externally".into();
    }
    data
}

fn sized_thumbnail(
    bytes: &[u8],
    mime: Option<&str>,
    width: u32,
    height: u32,
) -> Option<ImageThumbnail> {
    ImageThumbnail::generate_with_limits(
        bytes,
        mime,
        width,
        height,
        super::super::media::raster_preview_limits(),
    )
}

fn audio_excerpt(path: &Path) -> (Vec<f32>, Vec<[u8; 3]>) {
    let duration = little_clip_core::audio::read_audio_metadata(path)
        .map_or(0.0, |metadata| metadata.duration_ms as f32 / 1000.0);
    super::super::music::preview::waveform(path, duration, 64, Some(2.0), &AtomicBool::new(false))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_music_admission_is_bounded_and_retirement_cancels_every_job() {
        let dir = tempfile::tempdir().unwrap();
        let mut previews = Previews::default();
        for _ in 0..16 {
            previews.request(
                uuid::Uuid::new_v4(),
                Source::Music(dir.path().join("missing.wav")),
            );
        }
        assert_eq!(previews.pending.len(), music_preview_workers());
        let cancellation: Vec<_> = previews
            .pending
            .values()
            .map(|pending| pending.cancelled.clone())
            .collect();
        previews.retain(&BTreeSet::new());
        assert!(previews.pending.is_empty());
        assert!(
            cancellation
                .iter()
                .all(|token| token.load(Ordering::Relaxed))
        );
        previews.poll(&egui::Context::default());
        assert!(previews.ready.is_empty());
    }

    #[test]
    fn retired_preview_cannot_publish_a_stale_completion() {
        let ctx = egui::Context::default();
        let mut previews = Previews::default();
        let old = uuid::Uuid::new_v4();
        previews.request(
            old,
            Source::Payload(Payload::Content(
                super::super::super::content::Content::capture(b"old".to_vec(), "text/plain")
                    .unwrap(),
            )),
        );
        previews.retain(&BTreeSet::new());
        assert!(previews.pending.is_empty());
        previews.poll(&ctx);
        assert!(previews.get(old).is_none());
        let current = uuid::Uuid::new_v4();
        previews.request(
            current,
            Source::Payload(Payload::Content(
                super::super::super::content::Content::capture(b"current".to_vec(), "text/plain")
                    .unwrap(),
            )),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !previews.pending.is_empty() {
            previews.poll(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(previews.get(current).unwrap().data.text, "current");
        previews.invalidate(current);
        assert!(previews.get(current).is_none());
        assert!(previews.needs(current));
    }

    #[test]
    fn listing_previews_bound_rasters_and_text_excerpts() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("thumbnail.png");
        image::RgbaImage::from_pixel(640, 480, image::Rgba([20, 40, 80, 255]))
            .save(&image)
            .unwrap();
        let data = file_preview(&[image]);
        let thumbnail = data.image.unwrap();
        assert!(thumbnail.width <= 128 && thumbnail.height <= 80);
        assert!(thumbnail.premultiplied_rgba.len() <= 128 * 80 * 4);
        let text = dir.path().join("long.txt");
        std::fs::write(&text, "visible excerpt\n".repeat(100_000)).unwrap();
        let data = file_preview(&[text]);
        assert!(data.text.starts_with("visible excerpt"));
        assert!(data.text.len() <= 2048);
    }
}

#[cfg(test)]
mod gallery_completion_tests {
    use super::*;
    #[test]
    fn parallel_thumbnail_completions_keep_identity_and_retirement_changes_priority() {
        let ctx = egui::Context::default();
        let executor = BackgroundJobs::new(1, 8);
        let mut previews = Previews::default();
        let ids = [
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
        ];
        let mut senders = Vec::new();
        for id in ids {
            let (ticket, _) = executor.submit(Duration::from_secs(1), || ());
            let (sender, receiver) = mpsc::channel();
            previews.pending.insert(
                id,
                Pending {
                    ticket,
                    receiver,
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            );
            senders.push(sender);
        }
        senders[2]
            .send(Ok(Data::new("third".into(), "".into(), Icon::Image)))
            .unwrap();
        previews.poll(&ctx);
        assert_eq!(previews.get(ids[2]).unwrap().data.title, "third");
        assert!(previews.get(ids[0]).is_none());
        let obsolete = previews.pending[&ids[0]].cancelled.clone();
        previews.retain(&BTreeSet::from([ids[1], ids[2]]));
        assert!(obsolete.load(Ordering::Relaxed));
        assert!(
            senders[0]
                .send(Ok(Data::new("stale".into(), "".into(), Icon::Image)))
                .is_err()
        );
        senders[1]
            .send(Ok(Data::new("second".into(), "".into(), Icon::Image)))
            .unwrap();
        previews.poll(&ctx);
        assert_eq!(previews.get(ids[1]).unwrap().data.title, "second");
        assert!(previews.get(ids[0]).is_none());
    }
}
