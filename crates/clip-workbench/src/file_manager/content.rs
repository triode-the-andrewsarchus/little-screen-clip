//! Captured clipboard bytes have explicit encoders and never masquerade as paths.

use little_clip_core::{ClipItem, ClipPayload};
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
                if little_clip_core::kernel_interface::classify(path)
                    .map_err(|e| e.to_string())?
                    .protected()
                {
                    return files(vec![path.clone()]);
                }
                crate::file_source::read_file_bytes(path, MAX_CONTENT_BYTES, true, true)
                    .map_err(|e| e.to_string())?
            } else {
                return Err("Image bytes are no longer available".into());
            };
            Content::capture(bytes, mime).map(Payload::Content)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_item(bytes: Vec<u8>, path: Option<PathBuf>) -> ClipItem {
        let mut item = ClipItem::from_text("fixture");
        item.payload = ClipPayload::Image {
            bytes,
            path,
            width: 1,
            height: 1,
            mime: "image/svg+xml".into(),
            thumbnail: None,
        };
        item
    }

    #[test]
    fn protected_image_references_are_not_hydrated() {
        let root = tempfile::tempdir().unwrap();
        let alias = root.path().join("ordinary-looking.svg");
        std::os::unix::fs::symlink("/proc/uptime", &alias).unwrap();
        for path in [
            PathBuf::from("/proc/uptime"),
            PathBuf::from("/dev/zero"),
            alias,
        ] {
            let payload = from_item(&image_item(Vec::new(), Some(path.clone()))).unwrap();
            let Payload::Files { paths, cut } = payload else {
                panic!("a path-only image must stay a reference");
            };
            assert_eq!(paths, vec![path]);
            assert!(!cut);
        }
        assert!(from_item(&image_item(Vec::new(), None)).is_err());
        assert!(
            from_item(&image_item(
                Vec::new(),
                Some(root.path().join("missing.svg"))
            ))
            .is_err()
        );
    }

    #[test]
    fn selected_ordinary_image_hydrates_the_snapshot_omitted_from_ipc() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.svg");
        let bytes = b"<svg width='1' height='1'/>";
        std::fs::write(&path, bytes).unwrap();
        let payload = from_item(&image_item(Vec::new(), Some(path.clone()))).unwrap();
        let Payload::Content(content) = payload else {
            panic!("explicit ordinary-image collection must capture bytes");
        };
        assert_eq!(content.bytes.as_ref(), bytes);
        std::fs::write(&path, b"not the snapshot").unwrap();
        assert_eq!(content.bytes.as_ref(), bytes);
    }

    #[test]
    fn stored_image_snapshot_does_not_reopen_its_source_reference() {
        let bytes = b"<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'/>".to_vec();
        let payload = from_item(&image_item(bytes.clone(), Some("/proc/uptime".into()))).unwrap();
        let Payload::Content(content) = payload else {
            panic!("stored bytes must remain a captured snapshot");
        };
        assert_eq!(content.bytes.as_ref(), bytes);
        assert_eq!(content.mime, "image/svg+xml");
    }
}
