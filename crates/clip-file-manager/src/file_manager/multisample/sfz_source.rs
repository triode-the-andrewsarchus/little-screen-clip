//! Bounded textual SFZ includes and defines, through guarded immutable snapshots.
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
struct Preprocessor {
    files: usize,
    bytes: usize,
    output: String,
    defines: BTreeMap<String, String>,
}
pub(super) fn read(path: &Path, token: &AtomicBool) -> Result<String, String> {
    let mut reader = Preprocessor::default();
    reader.file(path, token)?;
    Ok(reader.output)
}
impl Preprocessor {
    fn file(&mut self, path: &Path, token: &AtomicBool) -> Result<(), String> {
        self.files += 1;
        if self.files > 32 {
            return Err("SFZ exceeds the 32-file include limit (or has an include cycle)".into());
        }
        if token.load(Ordering::Relaxed) {
            return Err("SFZ opening cancelled".into());
        }
        let bytes = crate::file_source::read_file_snapshot(path, 16 * 1024 * 1024)?;
        self.bytes += bytes.len();
        if self.bytes > 16 * 1024 * 1024 {
            return Err("SFZ includes exceed 16 MiB".into());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| "SFZ must be UTF-8 text")?;
        let text = little_clip_core::multisample::sfz::strip_comments(text)?;
        for line in text.lines() {
            self.line(line.trim(), path, token)?;
        }
        Ok(())
    }
    fn line(&mut self, line: &str, path: &Path, token: &AtomicBool) -> Result<(), String> {
        if let Some(include) = line.strip_prefix("#include") {
            let include = self.expand(include.trim())?;
            let include = include
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or("Expected a quoted SFZ include path")?;
            self.file(
                &path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(include.replace('\\', "/")),
                token,
            )?;
        } else if let Some(define) = line.strip_prefix("#define") {
            let (name, value) = define
                .trim()
                .split_once(char::is_whitespace)
                .ok_or("Malformed SFZ define")?;
            if !name.starts_with('$')
                || name.len() > 128
                || value.len() > 4096
                || self.defines.len() >= 128
            {
                return Err("SFZ define exceeds limits".into());
            }
            self.defines.insert(name.into(), value.trim().into());
        } else if line.starts_with('#') {
            return Err("Unsupported SFZ preprocessing directive".into());
        } else {
            let expanded = self.expand(line)?;
            if self.output.len() + expanded.len() > 16 * 1024 * 1024 {
                return Err("Expanded SFZ exceeds 16 MiB".into());
            }
            self.output.push_str(&expanded);
            self.output.push('\n');
        }
        Ok(())
    }
    fn expand(&self, text: &str) -> Result<String, String> {
        let mut result = text.to_owned();
        for _ in 0..8 {
            let mut expanded = String::new();
            let mut rest = result.as_str();
            while let Some(at) = rest.find('$') {
                expanded.push_str(&rest[..at]);
                let variable = &rest[at..];
                let length = 1 + variable[1..]
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    .count();
                let value = self
                    .defines
                    .get(&variable[..length])
                    .ok_or_else(|| format!("Undefined SFZ variable {}", &variable[..length]))?;
                if expanded.len() + value.len() + rest.len() > 16 * 1024 * 1024 {
                    return Err("Expanded SFZ variable exceeds limits".into());
                }
                expanded.push_str(value);
                rest = &variable[length..];
            }
            expanded.push_str(rest);
            result = expanded;
            if !result.contains('$') {
                return Ok(result);
            }
        }
        Err("SFZ variables are cyclic or nested more than eight levels".into())
    }
}
