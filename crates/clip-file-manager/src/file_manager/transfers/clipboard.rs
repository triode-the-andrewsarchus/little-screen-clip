use std::path::PathBuf;
use std::time::Duration;

use little_clip_core::{
    CommandRequest, copy_bytes_to_system_clipboard, copy_file_uris_to_system_clipboard,
    file_uri_list,
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
    let types = paste(&["--list-types"])?;
    let (mime, gnome) = reference_mime(&types)
        .ok_or_else(|| ReadError::Invalid("Clipboard contains no file references".into()))?;
    let payload = paste(&["--no-newline", "--type", mime])?;
    parse(&payload, gnome)
}

pub(in crate::file_manager) fn read_payload() -> Result<super::super::content::Payload, ReadError> {
    use super::super::content::{CONTENT_MIMES, Content, MAX_CONTENT_BYTES, Payload};
    let types = paste(&["--list-types"])?;
    let content_mime = CONTENT_MIMES
        .iter()
        .copied()
        .find(|mime| types.lines().any(|line| line == *mime));
    if let Some((mime, gnome)) = reference_mime(&types) {
        let payload = paste(&["--no-newline", "--type", mime])?;
        match parse(&payload, gnome) {
            Ok(references) => {
                return Ok(Payload::Files {
                    paths: references.paths,
                    cut: references.cut,
                });
            }
            // Browsers may offer a web URI alongside encoded image or PDF bytes.
            Err(error) => {
                let encoded = content_mime
                    .is_some_and(|mime| mime.starts_with("image/") || mime == "application/pdf");
                if gnome || !encoded {
                    return Err(error);
                }
            }
        }
    }
    let mime = content_mime.ok_or_else(|| {
        ReadError::Invalid(
            "Clipboard offers no supported file, text, image, or document format".into(),
        )
    })?;
    let bytes = paste_bytes(&["--no-newline", "--type", mime], MAX_CONTENT_BYTES)?;
    Content::capture(bytes, mime)
        .map(Payload::Content)
        .map_err(ReadError::Invalid)
}

fn reference_mime(types: &str) -> Option<(&'static str, bool)> {
    [
        ("x-special/gnome-copied-files", true),
        ("text/uri-list", false),
    ]
    .into_iter()
    .find(|(mime, _)| types.lines().any(|line| line == *mime))
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

fn paste(args: &[&str]) -> Result<String, ReadError> {
    String::from_utf8(paste_bytes(args, 1024 * 1024)?)
        .map_err(|e| ReadError::Invalid(e.to_string()))
}

fn paste_bytes(args: &[&str], limit: usize) -> Result<Vec<u8>, ReadError> {
    let output = CommandRequest::new("wl-paste")
        .args(args.iter().copied())
        .timeout(Duration::from_secs(5))
        .max_output_bytes(limit)
        .run()
        .map_err(|e| ReadError::Unavailable(e.to_string()))?;
    if !output.status.success() {
        return Err(ReadError::Unavailable(
            output.stderr_lossy().trim().to_owned(),
        ));
    }
    if output.stdout_truncated {
        return Err(ReadError::Invalid(format!(
            "Clipboard payload exceeds the {} MiB input limit",
            limit / (1024 * 1024)
        )));
    }
    Ok(output.stdout)
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
}
