//! Bounded notebook container reads; ZIP entry names are never extraction paths.
use std::{
    io::{Read, Seek},
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) fn member(
    input: &mut (impl Read + Seek),
    name: &str,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    input.rewind().map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(input).map_err(|error| error.to_string())?;
    zip_member(&mut archive, name, limit, cancel)
}

pub(crate) fn zip_member<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if archive.len() > 8192 {
        return Err("Notebook package exceeds the entry budget".into());
    }
    let mut file = archive
        .by_name(name)
        .map_err(|error| format!("Missing Notebook package member {name}: {error}"))?;
    if file.enclosed_name().is_none() || file.is_dir() || file.size() > limit as u64 {
        return Err("Notebook package member exceeds its path/size budget".into());
    }
    bounded(&mut file, limit, cancel)
}

pub(crate) fn bounded(
    input: &mut impl Read,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Notebook preview cancelled".into());
        }
        let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            return Ok(output);
        }
        if count > limit.saturating_sub(output.len()) {
            return Err("Expanded Notebook data exceeds its read budget".into());
        }
        output.extend_from_slice(&buffer[..count]);
    }
}
