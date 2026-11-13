//! Reversible Unix path addresses for display and literal input.
use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

/// Formats a reversible address, escaping backslashes, controls and invalid bytes.
///
/// Valid Unicode is preserved; invalid bytes and UTF-8 bytes of control characters
/// use `\xHH`. This is a literal address, not shell syntax.
pub fn display_path(path: &Path) -> String {
    display_path_cow(path).into_owned()
}

/// Borrows an ordinary Unicode address or returns an escaped owned address.
pub fn display_path_cow(path: &Path) -> std::borrow::Cow<'_, str> {
    if let Some(text) = path.to_str()
        && !text.chars().any(|ch| ch == '\\' || ch.is_control())
    {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut result = String::new();
    let mut bytes = path.as_os_str().as_bytes();
    while !bytes.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(bytes) {
            Ok(text) => (text, 0),
            Err(error) => (
                std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
                error
                    .error_len()
                    .unwrap_or(bytes.len() - error.valid_up_to()),
            ),
        };
        for ch in valid.chars() {
            if ch == '\\' {
                result.push_str("\\\\");
            } else if ch.is_control() {
                let mut encoded = [0; 4];
                for byte in ch.encode_utf8(&mut encoded).as_bytes() {
                    result.push_str(&format!("\\x{byte:02X}"));
                }
            } else {
                result.push(ch);
            }
        }
        bytes = &bytes[valid.len()..];
        for byte in &bytes[..invalid] {
            result.push_str(&format!("\\x{byte:02X}"));
        }
        bytes = &bytes[invalid..];
    }
    std::borrow::Cow::Owned(result)
}

/// Decodes a literal address produced by [`display_path`].
///
/// No tilde expansion, relative resolution or shell evaluation is performed.
///
/// # Errors
/// Returns an error for malformed escapes or an embedded NUL byte.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use little_clip_core::path_address::{decode_path, display_path};
/// let path = Path::new("/data/a\\b");
/// assert_eq!(decode_path(&display_path(path))?, path);
/// # Ok::<(), String>(())
/// ```
pub fn decode_path(text: &str) -> Result<PathBuf, String> {
    let mut decoded = Vec::new();
    let mut bytes = text.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        match bytes.next() {
            Some(b'\\') => decoded.push(b'\\'),
            Some(b'x') => {
                let hi = bytes.next().and_then(|b| (b as char).to_digit(16));
                let lo = bytes.next().and_then(|b| (b as char).to_digit(16));
                let (Some(hi), Some(lo)) = (hi, lo) else {
                    return Err("Invalid path escape: use \\xHH or \\\\".into());
                };
                decoded.push((hi * 16 + lo) as u8);
            }
            _ => return Err("Invalid path escape: use \\xHH or \\\\".into()),
        }
    }
    if decoded.contains(&0) {
        return Err("A path cannot contain a NUL byte".into());
    }
    Ok(PathBuf::from(OsString::from_vec(decoded)))
}

/// Decodes one filename using the reversible path-address syntax.
///
/// # Errors
/// Returns an error for invalid escapes, NUL bytes, empty names, path separators,
/// or the special directory components `.` and `..`.
pub fn decode_file_name(text: &str) -> Result<OsString, String> {
    let path = decode_path(text)?;
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty()
        || bytes.contains(&b'/')
        || path == Path::new(".")
        || path == Path::new("..")
    {
        return Err("Enter one filename, without /, . or ..".into());
    }
    Ok(path.into_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_round_trip_bytes_and_reject_paths() {
        for invalid in ["", ".", "..", "../escape", "a/b", "\\x00"] {
            assert!(decode_file_name(invalid).is_err(), "{invalid}");
        }
        let name = decode_file_name("space \\xFF\\\\end").unwrap();
        assert_eq!(name.as_bytes(), b"space \xff\\end");
    }
}
