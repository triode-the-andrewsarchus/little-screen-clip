//! Captured clipboard bytes have explicit encoders and never masquerade as paths.

use little_clip_core::{ClipItem, ClipPayload};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) const MAX_CONTENT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Content {
    pub bytes: Arc<[u8]>,
    pub mime: String,
    pub extension: String,
}

impl Content {
    pub fn capture(bytes: Vec<u8>, mime: &str) -> Result<Self, String> {
        if bytes.len() > MAX_CONTENT_BYTES {
            return Err("Clipboard content exceeds 64 MiB".into());
        }
        let extension = extension(mime).ok_or_else(|| format!("No file encoder for {mime}"))?;
        if mime.starts_with("text/")
            || matches!(mime, "application/json" | "application/xml" | "UTF8_STRING")
        {
            std::str::from_utf8(&bytes).map_err(|_| "Clipboard text is not valid UTF-8")?;
        }
        if mime.starts_with("image/")
            && mime != "image/svg+xml"
            && little_clip_core::item::read_image_dimensions_with_mime(&bytes, Some(mime)).is_none()
        {
            return Err("Clipboard image has invalid or unsupported encoded data".into());
        }
        Ok(Self {
            bytes: bytes.into(),
            mime: mime.into(),
            extension: extension.into(),
        })
    }

    pub fn default_name(&self) -> String {
        format!("Clipboard.{}", self.extension)
    }
}

pub(super) const CONTENT_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/svg+xml",
    "application/pdf",
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "text/markdown",
    "text/csv",
    "application/json",
    "text/html",
    "application/xml",
    "text/xml",
];

fn extension(mime: &str) -> Option<&'static str> {
    match mime {
        "text/plain" | "text/plain;charset=utf-8" | "UTF8_STRING" => Some("txt"),
        "text/markdown" => Some("md"),
        "text/csv" => Some("csv"),
        "text/html" => Some("html"),
        "application/json" => Some("json"),
        "application/xml" | "text/xml" => Some("xml"),
        "application/pdf" => Some("pdf"),
        _ => little_clip_core::item::image_extension_for_mime(mime),
    }
}

#[derive(Clone)]
pub(super) enum Payload {
    Files { paths: Vec<PathBuf>, cut: bool },
    Content(Content),
}

pub(super) fn from_item(item: &ClipItem) -> Result<Payload, String> {
    let content = |text: &str, mime: &str| {
        Content::capture(text.as_bytes().to_vec(), mime).map(Payload::Content)
    };
    let files = |paths: Vec<PathBuf>| Ok(Payload::Files { paths, cut: false });
    match &item.payload {
        ClipPayload::Text(text) => content(text, "text/plain"),
        ClipPayload::Markdown(text) => content(text, "text/markdown"),
        ClipPayload::Url(url) => match url.to_file_path() {
            Ok(path) => files(vec![path]),
            Err(_) => content(url.as_str(), "text/plain"),
        },
        ClipPayload::Color { hex, .. } => content(hex, "text/plain"),
        ClipPayload::FileBatch(paths) | ClipPayload::MultiImage { paths, .. } => {
            files(paths.clone())
        }
        ClipPayload::Directory(path)
        | ClipPayload::AudioRef { path, .. }
        | ClipPayload::Animation { path, .. } => files(vec![path.clone()]),
        ClipPayload::Image {
            bytes, path, mime, ..
        } => {
            let bytes = if !bytes.is_empty() {
                bytes.clone()
            } else if let Some(path) = path {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
                    .map_err(|e| e.to_string())?;
                if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                    return Err("Image content source is not a regular file".into());
                }
                let mut bytes = Vec::new();
                file.take(MAX_CONTENT_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                bytes
            } else {
                return Err("Image bytes are no longer available".into());
            };
            Content::capture(bytes, mime).map(Payload::Content)
        }
    }
}
