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
    let gnome = types
        .lines()
        .any(|line| line == "x-special/gnome-copied-files");
    if !gnome && !types.lines().any(|line| line == "text/uri-list") {
        return Err(ReadError::Invalid(
            "Clipboard contains no file references".into(),
        ));
    }
    let mime = if gnome {
        "x-special/gnome-copied-files"
    } else {
        "text/uri-list"
    };
    let payload = paste(&["--no-newline", "--type", mime])?;
    parse(&payload, gnome)
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
    let output = CommandRequest::new("wl-paste")
        .args(args.iter().copied())
        .timeout(Duration::from_secs(5))
        .max_output_bytes(1024 * 1024)
        .run()
        .map_err(|e| ReadError::Unavailable(e.to_string()))?;
    if !output.status.success() {
        return Err(ReadError::Unavailable(
            output.stderr_lossy().trim().to_owned(),
        ));
    }
    if output.stdout_truncated {
        return Err(ReadError::Invalid(
            "Clipboard file references exceed the 1 MiB input limit".into(),
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| ReadError::Invalid(e.to_string()))
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
