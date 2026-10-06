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
    Gallery(PathBuf),
    Music(PathBuf),
}

pub(in crate::file_manager) struct Data {
    pub title: String,
    pub detail: String,
    pub text: String,
    pub image: Option<ImageThumbnail>,
    pub waveform: Vec<f32>,
    pub icon: Icon,
    pub music: Option<super::super::music::preview::Metadata>,
}

impl Data {
    pub(in crate::file_manager) fn new(title: String, detail: String, icon: Icon) -> Self {
        Self {
            title,
            detail,
            icon,
            text: String::new(),
            image: None,
            waveform: Vec::new(),
            music: None,
        }
    }
}

pub(in crate::file_manager) struct Ready {
    pub data: Data,
    pub texture: Option<egui::TextureHandle>,
}
struct Pending {
    id: uuid::Uuid,
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
    pending: Option<Pending>,
}

impl Previews {
    pub fn needs(&self, id: uuid::Uuid) -> bool {
        self.pending.is_none() && !self.ready.contains_key(&id)
    }
    pub fn clear(&mut self) {
        self.ready.clear();
        if let Some(pending) = self.pending.take() {
            pending.ticket.cancel();
        }
    }
    pub fn invalidate(&mut self, id: uuid::Uuid) {
        self.ready.remove(&id);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            self.pending.take().unwrap().ticket.cancel();
        }
    }

    pub fn retain(&mut self, ids: &BTreeSet<uuid::Uuid>) {
        self.ready.retain(|id, _| ids.contains(id));
        if let Some(pending) = &self.pending
            && !ids.contains(&pending.id)
        {
            self.invalidate(pending.id);
        }
    }

    pub fn get(&self, id: uuid::Uuid) -> Option<&Ready> {
        self.ready.get(&id)
    }

    pub fn request(&mut self, id: uuid::Uuid, source: Source) {
        if self.ready.contains_key(&id) || self.pending.is_some() {
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
            BackgroundJobs::global().submit(Duration::from_secs(deadline), move || match source {
                Source::Payload(payload) => payload_preview(&payload),
                Source::Slot(slot, expected) => slot_preview(slot, expected),
                Source::Gallery(path) => gallery_preview(&path),
                Source::Music(path) => super::super::music::preview::load(&path, &token),
            });
        self.pending = Some(Pending {
            id,
            ticket,
            receiver,
            cancelled,
        });
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else {
            return;
        };
        let result = match pending.receiver.try_recv() {
            Ok(Err(JobError::QueueFull)) => {
                self.pending = None;
                ctx.request_repaint();
                return;
            }
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            Err(_) => Err(JobError::Cancelled),
        };
        let id = self.pending.take().unwrap().id;
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
        data.waveform = audio_excerpt(path);
    } else if little_clip_core::is_video_path(path) {
        data.icon = Icon::Film;
        data.detail = "Video".into();
        data.image = super::super::viewers::video::preview(path);
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

fn gallery_preview(path: &Path) -> Data {
    let mut data = Data::new(display_path(path), "Photo".into(), Icon::Image);
    data.image = crate::file_source::read_file_bytes(path, 64 * 1024 * 1024, true, false)
        .ok()
        .and_then(|bytes| {
            sized_thumbnail(
                &bytes,
                Some(&little_clip_core::item::guess_image_mime(path)),
                320,
                320,
            )
        });
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
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    ImageThumbnail::generate_with_limits(bytes, mime, width, height, limits)
}

fn audio_excerpt(path: &Path) -> Vec<f32> {
    if !little_clip_core::audio::read_audio_metadata(path)
        .is_some_and(|metadata| metadata.duration_ms > 0)
    {
        return Vec::new();
    }
    let config = little_clip_core::config::AudioConfig {
        max_full_waveform_sec: 0,
        snippet_duration_sec: 2,
        sample_position: little_clip_core::config::AudioSamplePosition::Start,
        ..Default::default()
    };
    little_clip_core::audio::compute_plain_waveform(path, 64, &config)
        .map(|waveform| {
            waveform
                .ch1_peaks
                .into_iter()
                .take(64)
                .map(|peak| {
                    if peak.is_finite() {
                        peak.clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(previews.pending.is_none());
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
        while previews.pending.is_some() {
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
