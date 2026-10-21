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
    Photo {
        path: PathBuf,
        revision: u64,
    },
    Music(PathBuf, bool),
    Text(PathBuf),
    System(PathBuf),
    Binary(PathBuf),
    Font(PathBuf),
    FontRaster {
        document: Arc<super::super::font_strip::Document>,
        width: u32,
        height: u32,
        color: [u8; 3],
    },
    BinaryRaster {
        document: Arc<super::super::binary_strip::Document>,
        width: u32,
        height: u32,
    },
    Midi(PathBuf),
    Blend(PathBuf),
    Material(super::super::material_strip::Request),
    MidiRaster {
        document: Arc<super::super::midi_strip::Document>,
        width: u32,
        height: u32,
        colors: Vec<[u8; 3]>,
    },
    PdfPage {
        path: PathBuf,
        page: usize,
        edge: u32,
    },
    TextRaster {
        document: Arc<super::super::text_strip::Document>,
        width: u32,
        height: u32,
        colors: [[u8; 3]; 9],
    },
}

impl Source {
    fn execution(&self) -> (&'static BackgroundJobs, usize, u64) {
        match self {
            Self::System(_) => (super::super::system::jobs(), 2, 3),
            Self::Music(..) => (music_preview_jobs(), music_preview_workers(), 30),
            Self::Gallery { .. } | Self::PdfPage { .. } | Self::Photo { .. } | Self::Blend(_) => {
                (gallery_preview_jobs(), 4, 15)
            }
            Self::Text(_)
            | Self::Font(_)
            | Self::FontRaster { .. }
            | Self::Binary(_)
            | Self::BinaryRaster { .. }
            | Self::TextRaster { .. }
            | Self::Midi(_)
            | Self::MidiRaster { .. }
            | Self::Material(_) => (text_preview_jobs(), 4, 30),
            _ => (BackgroundJobs::global(), 1, 15),
        }
    }
    fn load(self, token: &AtomicBool) -> Data {
        match self {
            Source::Payload(payload) => payload_preview(&payload),
            Source::Slot(slot, expected) => slot_preview(slot, expected),
            Source::Gallery {
                path,
                edge,
                orientation,
            } => gallery_preview(&path, edge, orientation, token),
            Source::Photo { path, revision } => {
                super::super::image_strip::load(&path, revision, token)
            }
            Source::Music(path, spectrum) => {
                super::super::music::preview::load(&path, spectrum, token)
            }
            Source::Blend(path) => super::super::blend_preview::load(&path, token),
            Source::Material(request) => request.load(token),
            Source::PdfPage { path, page, edge } => {
                super::super::pdf_strip::load_page(&path, page, edge, token)
            }
            Source::Midi(path) => {
                let mut data = Data::new(display_path(&path), "MIDI".into(), Icon::Midi);
                match super::super::midi_strip::load(&path, token) {
                    Ok(document) => {
                        data.detail = document.details();
                        data.midi_document = Some(document);
                    }
                    Err(error) => {
                        data.detail = error;
                    }
                }
                data
            }
            Source::MidiRaster {
                document,
                width,
                height,
                colors,
            } => {
                let mut data = Data::new(String::new(), document.details(), Icon::Midi);
                data.image =
                    super::super::midi_strip::render(&document, width, height, &colors, token);
                data.midi_document = Some(document);
                data
            }
            Source::System(path) => super::super::system::load(&path, token),
            Source::Font(path) => super::super::font_strip::load(&path, token),
            Source::FontRaster {
                document,
                width,
                height,
                color,
            } => {
                let mut data = Data::new(String::new(), String::new(), Icon::Text);
                data.image =
                    super::super::font_strip::render(&document, width, height, color, token);
                data
            }
            Source::Binary(path) => super::super::binary_strip::load(&path, token),
            Source::BinaryRaster {
                document,
                width,
                height,
            } => {
                let mut data = Data::new(String::new(), String::new(), Icon::Text);
                data.image = super::super::binary_strip::render(&document, width, height, token);
                data
            }
            Source::Text(path) => document_preview(&path, token),
            Source::TextRaster {
                document,
                width,
                height,
                colors,
            } => {
                let mut data = Data::new(String::new(), String::new(), Icon::Text);
                data.image =
                    super::super::text_strip::render(&document, width, height, colors, token);
                data.document = Some(document);
                data
            }
        }
    }
}

pub(in crate::file_manager) use super::super::media::PreviewData as Data;

pub(in crate::file_manager) struct Ready {
    pub data: Data,
    pub texture: Option<egui::TextureHandle>,
    pub spectrum_texture: Option<egui::TextureHandle>,
    pub hash_textures: [Option<egui::TextureHandle>; 2],
}

/// One raster hover job, replaced when its source, position, or size changes.
#[derive(Default)]
pub(in crate::file_manager) struct HoverRaster {
    cache: Previews,
    key: Option<(uuid::Uuid, usize, u32, uuid::Uuid)>,
    seen: bool,
}
impl HoverRaster {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
    }
    pub fn retain(&mut self, keep: impl Fn(uuid::Uuid) -> bool) {
        if !self.seen || self.key.is_some_and(|(source, _, _, _)| !keep(source)) {
            self.clear();
        }
        self.seen = false;
    }
    pub fn request(
        &mut self,
        identity: uuid::Uuid,
        position: usize,
        edge: u32,
        source: Source,
    ) -> Option<&Ready> {
        self.seen = true;
        if !self
            .key
            .is_some_and(|key| key.0 == identity && key.1 == position && key.2 == edge)
        {
            self.cache.clear();
            self.key = Some((identity, position, edge, uuid::Uuid::new_v4()));
        }
        let id = self.key.unwrap().3;
        if self.cache.needs(id) {
            self.cache.request(id, source);
        }
        self.cache.get(id)
    }
}
struct Pending {
    response_deadline: Option<std::time::Instant>,
    executor: &'static BackgroundJobs,
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
        let _ = self.start(id, source, false);
    }
    pub fn refresh(&mut self, id: uuid::Uuid, source: Source) -> bool {
        self.start(id, source, true)
    }
    fn start(&mut self, id: uuid::Uuid, source: Source, refresh: bool) -> bool {
        let (executor, limit, deadline) = source.execution();
        if self.pending.contains_key(&id)
            || (!refresh && self.ready.contains_key(&id))
            || self
                .pending
                .values()
                .filter(|pending| std::ptr::eq(pending.executor, executor))
                .count()
                >= limit
        {
            return false;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = cancelled.clone();
        let response_deadline = matches!(&source, Source::System(_))
            .then(|| std::time::Instant::now() + super::super::system::RESPONSE_DEADLINE);
        let (ticket, receiver) =
            executor.submit(Duration::from_secs(deadline), move || source.load(&token));
        self.pending.insert(
            id,
            Pending {
                response_deadline,
                executor,
                ticket,
                receiver,
                cancelled,
            },
        );
        true
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut completed = Vec::new();
        self.pending.retain(|id, pending| {
            let result =
                match super::super::preview_job::poll(&pending.receiver, pending.response_deadline)
                {
                    Some(Err(JobError::QueueFull)) => {
                        ctx.request_repaint();
                        return false;
                    }
                    Some(result) => result,
                    None => return true,
                };
            completed.push((*id, result));
            false
        });
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let changed = !completed.is_empty();
        for (id, result) in completed {
            self.complete(ctx, id, result);
        }
        changed
    }

    fn complete(&mut self, ctx: &egui::Context, id: uuid::Uuid, result: Result<Data, JobError>) {
        let mut data = result.unwrap_or_else(|error| {
            let mut data = Data::new(
                "Preview unavailable".into(),
                "Retry with Refresh previews".into(),
                Icon::Alert,
            );
            data.text = error.to_string();
            data
        });
        if data.system.is_none() {
            if let Some(previous) = self.ready.get(&id).and_then(|r| r.data.system.as_ref()) {
                let mut retained = (**previous).clone();
                retained.last_error = Some(data.detail.clone());
                data.detail = retained.title.clone();
                data.system = Some(Arc::new(retained));
            }
        }
        if let Some((current, previous)) = data.system.as_mut().zip(
            self.ready
                .get(&id)
                .and_then(|ready| ready.data.system.as_ref()),
        ) {
            if current
                .facts
                .iter()
                .any(|fact| fact.retained && fact.observed_generation.is_none())
            {
                Arc::make_mut(current).inherit_retained_provenance(previous);
            }
        }
        let options = if data.document.is_some() || data.midi_document.is_some() {
            egui::TextureOptions::NEAREST
        } else {
            egui::TextureOptions::LINEAR
        };
        let texture = data
            .image
            .as_ref()
            .map(|image| upload_texture(ctx, format!("shelf_preview_{id}"), image, options));
        let spectrum_texture = data.spectrum.as_ref().map(|image| {
            upload_texture(
                ctx,
                format!("music_spectrum_{id}"),
                image,
                egui::TextureOptions::LINEAR,
            )
        });
        let hash_textures = std::array::from_fn(|index| {
            data.photo.as_ref()?.hash_art[index].as_ref().map(|image| {
                upload_texture(
                    ctx,
                    format!("image_hash_{id}_{index}"),
                    image,
                    if index == 1 {
                        egui::TextureOptions::NEAREST
                    } else {
                        egui::TextureOptions::LINEAR
                    },
                )
            })
        });
        self.ready.insert(
            id,
            Ready {
                data,
                texture,
                hash_textures,
                spectrum_texture,
            },
        );
        ctx.request_repaint();
    }
}

fn upload_texture(
    ctx: &egui::Context,
    name: String,
    image: &ImageThumbnail,
    options: egui::TextureOptions,
) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_premultiplied(
            [image.width as usize, image.height as usize],
            &image.premultiplied_rgba,
        ),
        options,
    )
}

fn text_preview_jobs() -> &'static BackgroundJobs {
    static JOBS: std::sync::LazyLock<BackgroundJobs> =
        std::sync::LazyLock::new(|| BackgroundJobs::new(4, 8));
    &JOBS
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
        Payload::Content(content) if little_clip_core::document::is_office_mime(&content.mime) => {
            Data::new(content.default_name(), "Office document".into(), Icon::Pdf)
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
    if little_clip_core::kernel_interface::require_content(path).is_err() {
        data.detail = "Kernel interface reference; contents not captured".into();
        return;
    }
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
            data.text = info.title.clone().unwrap_or_default();
            data.pdf_info = Some(Arc::new(info));
        }
    } else if little_clip_core::document::is_office_document(path) {
        data.icon = Icon::Pdf;
        if let Ok(pdf_path) = little_clip_core::document::ensure_rendered_pdf(path) {
            if let Ok(info) = little_clip_core::pdf::get_pdf_info(&pdf_path) {
                data.detail = format!("{} pages", info.page_count);
                data.text = info.title.clone().unwrap_or_default();
                data.pdf_info = Some(Arc::new(info));
            }
        }
        if data.pdf_info.is_none() {
            if let Ok(md) = little_clip_core::document::render_to_markdown(path) {
                data.icon = Icon::Text;
                data.text = md.lines().take(4).collect::<Vec<_>>().join("\n");
                data.detail = "Document".to_string();
            }
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
    fn row_completion_preserves_retained_fact_provenance_without_cloning_healthy_snapshots() {
        let ctx = egui::Context::default();
        let id = uuid::Uuid::new_v4();
        let mut previews = Previews::default();
        let previous = super::super::super::system::retained_provenance_fixture();
        let previous = Arc::new(previous);
        let mut data = Data::new("system".into(), "".into(), Icon::Text);
        data.system = Some(previous.clone());
        previews.complete(&ctx, id, Ok(data));
        assert!(Arc::ptr_eq(
            previews.get(id).unwrap().data.system.as_ref().unwrap(),
            &previous
        ));

        let mut retained = (*previous).clone();
        retained.facts[0].retained = true;
        retained.facts[0].observed_generation = None;
        retained.facts[0].error = Some("refresh failed".into());
        let retained = Arc::new(retained);
        let mut data = Data::new("system".into(), "".into(), Icon::Text);
        data.system = Some(retained.clone());
        previews.complete(&ctx, id, Ok(data));
        let current = previews.get(id).unwrap().data.system.as_ref().unwrap();
        assert_eq!(current.facts[0].observed_generation, Some(40));
        assert_eq!(current.facts[0].observed_ms, 123_000);
        assert!(current.facts[0].retained);
        assert_eq!(current.facts[0].error.as_deref(), Some("refresh failed"));
        assert_eq!(retained.facts[0].observed_generation, None);
    }

    #[test]
    fn parallel_media_admission_is_bounded_and_retirement_cancels_every_job() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "example\n").unwrap();
        let document =
            super::super::super::text_strip::load(&text, &AtomicBool::new(false)).unwrap();
        let cases = [
            (
                Source::Music(dir.path().join("missing.wav"), false),
                music_preview_workers(),
            ),
            (Source::Text(text), 4),
            (
                Source::TextRaster {
                    document,
                    width: 20,
                    height: 20,
                    colors: [[200; 3]; 9],
                },
                4,
            ),
        ];
        for (source, limit) in cases {
            let mut previews = Previews::default();
            for _ in 0..16 {
                previews.request(uuid::Uuid::new_v4(), source.clone());
            }
            assert_eq!(previews.pending.len(), limit);
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
                    response_deadline: None,
                    ticket,
                    receiver,
                    cancelled: Arc::new(AtomicBool::new(false)),
                    executor: BackgroundJobs::global(),
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

fn document_preview(path: &Path, token: &AtomicBool) -> Data {
    if little_clip_core::document::is_office_document(path) {
        let mut data = Data::new(display_path(path), "Document".into(), Icon::Pdf);
        preview_regular_file(path, &mut data);
        return data;
    }
    if let Ok(prefix) = crate::file_source::read_file_bytes(path, 8192, false, true) {
        let non_text = prefix
            .iter()
            .any(|&byte| byte == 0 || (byte < 32 && !matches!(byte, b'\n' | b'\r' | b'\t' | 12)));
        if non_text {
            return super::super::binary_strip::load(path, token);
        }
    }
    let mut data = Data::new(display_path(path), "Text".into(), Icon::Text);
    match super::super::text_strip::load(path, token) {
        Ok(document) => {
            data.document = Some(document);
        }
        Err(error) if error == "Binary file" || error == "Unrecognized non-UTF-8 file" => {
            return super::super::binary_strip::load(path, token);
        }
        Err(error) => data.detail = error,
    }
    data
}
