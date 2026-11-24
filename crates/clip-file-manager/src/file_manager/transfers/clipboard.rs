use std::path::PathBuf;

use little_clip_core::{
    ClipboardError, ClipboardSelection, copy_bytes_to_system_clipboard,
    copy_file_uris_to_system_clipboard, file_uri_list, list_system_clipboard_types,
    read_system_clipboard_bytes,
};

pub(crate) struct FileReferences {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

pub(super) fn write(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    if cut {
        let payload = format!("cut\n{}", file_uri_list(paths).map_err(|e| e.to_string())?);
        copy_bytes_to_system_clipboard(payload.as_bytes(), "x-special/gnome-copied-files")
            .map_err(|e| e.to_string())
    } else {
        copy_file_uris_to_system_clipboard(paths).map_err(|e| e.to_string())
    }
}

pub(super) fn read() -> Result<FileReferences, ReadError> {
    let types = list_system_clipboard_types(ClipboardSelection::Clipboard).map_err(map_error)?;
    let (mime, gnome) = reference_mime(&types)
        .ok_or_else(|| ReadError::Invalid("Clipboard contains no file references".into()))?;
    let payload = paste_text(mime)?;
    parse(&payload, gnome)
}

pub(in crate::file_manager) fn read_payload() -> Result<super::super::content::Payload, ReadError> {
    use super::super::content::{Content, MAX_CONTENT_BYTES, Payload};
    let types = list_system_clipboard_types(ClipboardSelection::Clipboard).map_err(map_error)?;
    let selected_content = select_content_target(&types);
    if let Some((mime, gnome)) = reference_mime(&types) {
        let payload = paste_text(mime)?;
        match parse(&payload, gnome) {
            Ok(references) => {
                return Ok(Payload::Files {
                    paths: references.paths,
                    cut: references.cut,
                });
            }
            // Browsers may offer a web URI alongside encoded image or PDF bytes.
            Err(error) => {
                let encoded = selected_content.is_some_and(|(_, canonical)| {
                    canonical.starts_with("image/") || canonical == "application/pdf"
                });
                if gnome || !encoded {
                    return Err(error);
                }
            }
        }
    }
    let (offered, canonical) = selected_content.ok_or_else(|| {
        ReadError::Invalid(
            "Clipboard offers no supported file, text, image, or document format".into(),
        )
    })?;
    let mut bytes = read_system_clipboard_bytes(
        ClipboardSelection::Clipboard,
        Some(offered),
        MAX_CONTENT_BYTES,
    )
    .map_err(map_error)?;
    if offered == "STRING" {
        bytes = bytes
            .into_iter()
            .map(|b| b as char)
            .collect::<String>()
            .into_bytes();
    }
    Content::capture(bytes, canonical)
        .map(Payload::Content)
        .map_err(ReadError::Invalid)
}

fn reference_mime(types: &[String]) -> Option<(&'static str, bool)> {
    [
        ("x-special/gnome-copied-files", true),
        ("text/uri-list", false),
    ]
    .into_iter()
    .find(|(mime, _)| types.iter().any(|line| line == *mime))
}

fn select_content_target(types: &[String]) -> Option<(&str, &'static str)> {
    use super::super::content::CONTENT_MIMES;
    for &mime in CONTENT_MIMES {
        if let Some(offered) = types.iter().find(|target| {
            *target == mime
                || (mime == "text/plain;charset=utf-8" && target.eq_ignore_ascii_case(mime))
        }) {
            return Some((offered.as_str(), mime));
        }
    }
    types
        .iter()
        .any(|target| target == "STRING")
        .then_some(("STRING", "text/plain"))
}

pub(crate) fn parse(payload: &str, gnome: bool) -> Result<FileReferences, ReadError> {
    let mut lines = payload.lines();
    let cut = if gnome {
        match lines.next() {
            Some("cut") => true,
            Some("copy") => false,
            _ => {
                return Err(ReadError::Invalid(
                    "Invalid file-reference operation".into(),
                ));
            }
        }
    } else {
        false
    };
    let mut paths = Vec::new();
    for line in lines {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let uri = url::Url::parse(line).map_err(|e| ReadError::Invalid(e.to_string()))?;
        let path = uri
            .to_file_path()
            .map_err(|_| ReadError::Invalid("Only local file references can be pasted".into()))?;
        paths.push(path);
    }
    if paths.is_empty() {
        return Err(ReadError::Invalid(
            "Clipboard contains no local file references".into(),
        ));
    }
    Ok(FileReferences { paths, cut })
}

fn paste_text(mime: &str) -> Result<String, ReadError> {
    let bytes = read_system_clipboard_bytes(ClipboardSelection::Clipboard, Some(mime), 1024 * 1024)
        .map_err(map_error)?;
    String::from_utf8(bytes).map_err(|e| ReadError::Invalid(e.to_string()))
}

fn map_error(error: ClipboardError) -> ReadError {
    match error {
        ClipboardError::Truncated(limit) => ReadError::Invalid(format!(
            "Clipboard payload exceeds the {} MiB input limit",
            limit / (1024 * 1024)
        )),
        ClipboardError::InvalidPath(message) => ReadError::Invalid(message),
        ClipboardError::Unavailable | ClipboardError::Failed(_) => {
            ReadError::Unavailable(error.to_string())
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Invalid(String),
}

pub(super) fn consume_cut(previous: &[PathBuf], remaining: &[PathBuf]) -> Result<(), String> {
    match read() {
        Ok(references) if references.cut && references.paths == previous => {
            if remaining.is_empty() {
                little_clip_core::copy_text_to_system_clipboard("").map_err(|e| e.to_string())
            } else {
                write(remaining, true)
            }
        }
        Ok(_) | Err(ReadError::Invalid(_)) => Ok(()), // Another application owns a newer selection.
        Err(ReadError::Unavailable(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    #[test]
    fn local_uri_references_round_trip_spaces_unicode_and_invalid_bytes() {
        let paths = vec![
            PathBuf::from("/tmp/space name 日本語"),
            PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/invalid\xff".to_vec())),
        ];
        let payload = file_uri_list(&paths).unwrap();
        assert_eq!(parse(&payload, false).unwrap().paths, paths);
        assert!(parse(&format!("cut\n{payload}"), true).unwrap().cut);
        assert!(!parse(&format!("copy\n{payload}"), true).unwrap().cut);
    }
    #[test]
    fn invalid_or_remote_references_are_rejected_as_a_whole() {
        for payload in [
            "",
            "# comment",
            "https://example.com/file",
            "file://remote/tmp/file",
            "file:///tmp/good\ninvalid",
        ] {
            assert!(parse(payload, false).is_err(), "{payload}");
        }
        assert!(parse("move\nfile:///tmp/file", true).is_err());
    }
    #[test]
    fn x11_content_target_selection_handles_utf8_charset_case_and_string_fallback() {
        let targets: Vec<String> = vec![
            "TARGETS".into(),
            "text/plain;charset=UTF-8".into(),
            "UTF8_STRING".into(),
        ];
        assert_eq!(
            select_content_target(&targets),
            Some(("text/plain;charset=UTF-8", "text/plain;charset=utf-8"))
        );

        let string_only: Vec<String> = vec!["TARGETS".into(), "STRING".into()];
        assert_eq!(
            select_content_target(&string_only),
            Some(("STRING", "text/plain"))
        );

        let image_and_text: Vec<String> = vec!["UTF8_STRING".into(), "image/png".into()];
        assert_eq!(
            select_content_target(&image_and_text),
            Some(("image/png", "image/png"))
        );
    }
}
