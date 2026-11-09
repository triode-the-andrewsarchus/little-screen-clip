//! Retained sampler archives: read mappings first, decode only a chosen member.
use crate::file_source::{FileSource, open_regular_file};
use little_clip_core::{
    multisample::{Document, EmbeddedSample},
    zip_content,
};
use std::{
    fs::File,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone)]
pub(super) struct Archive {
    zip: Arc<Mutex<zip::ZipArchive<File>>>,
    held: Arc<File>,
    expected: FileSource,
}
impl Archive {
    pub fn relocate(&mut self, source: FileSource) {
        self.expected = source;
    }
    pub fn member(&self, name: &str, token: &AtomicBool) -> Result<Vec<u8>, String> {
        cancelled(token)?;
        let mut zip = self
            .zip
            .lock()
            .map_err(|_| "Sample archive reader failed")?;
        self.check()?;
        let bytes = zip_content::entry(&mut zip, name, 64 * 1024 * 1024)?;
        self.check()?;
        cancelled(token)?;
        Ok(bytes)
    }
    fn check(&self) -> Result<(), String> {
        if FileSource::capture_file(&self.held)? != self.expected {
            return Err("Sample archive changed; reopen the instrument".into());
        }
        self.expected.check()
    }
}
pub(super) fn read(path: &Path, token: &AtomicBool) -> Result<(Document, Archive), String> {
    cancelled(token)?;
    let file = open_regular_file(path, true).map_err(|e| e.to_string())?;
    let expected = FileSource::capture_file(&file)?;
    let held = Arc::new(file.try_clone().map_err(|e| e.to_string())?);
    let mut zip = zip_content::archive_file(file, 2 * 1024 * 1024 * 1024)?;
    let manifests = manifests(&zip, path)?;
    let mut document = Document {
        instruments: Vec::new(),
        notes: Vec::new(),
        pcm: None,
    };
    let mut expanded = 0;
    let mut region_count = 0;
    for manifest in manifests {
        cancelled(token)?;
        let bytes = zip_content::entry(&mut zip, &manifest, 4 * 1024 * 1024 - expanded)?;
        expanded += bytes.len();
        let name = Path::new(&manifest)
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("Preset");
        let parsed = parse_manifest(&zip, &manifest, &bytes, name)?;
        merge(&mut document, parsed, &manifest, &mut region_count)?;
    }
    document.notes.push("Embedded audio is decoded only when selected. Archive members are never extracted into your folders.".into());
    document.validate()?;
    let archive = Archive {
        zip: Arc::new(Mutex::new(zip)),
        held,
        expected,
    };
    archive.check()?;
    Ok((document, archive))
}
fn parse_manifest(
    zip: &zip::ZipArchive<File>,
    manifest: &str,
    bytes: &[u8],
    name: &str,
) -> Result<Document, String> {
    if manifest == "Instrument.xml" {
        little_clip_core::multisample::xml::parse_renoise(bytes, name, &renoise_members(zip)?)
    } else {
        little_clip_core::multisample::xml::parse(bytes, name)
    }
}
fn merge(
    document: &mut Document,
    mut parsed: Document,
    manifest: &str,
    count: &mut usize,
) -> Result<(), String> {
    for instrument in &mut parsed.instruments {
        *count += instrument.regions.len();
        if *count > 8192 {
            return Err("Archive has more than 8,192 sample regions".into());
        }
        for region in &mut instrument.regions {
            let path = local_member(manifest, &region.sample)?;
            region.embedded = Some(EmbeddedSample::Archive { path });
        }
    }
    document.instruments.extend(parsed.instruments);
    if document.notes.is_empty() {
        document.notes = parsed.notes;
    }
    document.validate()
}
fn manifests(zip: &zip::ZipArchive<File>, path: &Path) -> Result<Vec<String>, String> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xrni"))
    {
        return Ok(vec!["Instrument.xml".into()]);
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("multisample"))
    {
        return Ok(vec!["multisample.xml".into()]);
    }
    let mut entries: Vec<String> = zip
        .file_names()
        .filter(|n| {
            Path::new(n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dspreset"))
        })
        .map(str::to_owned)
        .collect();
    if entries.is_empty() || entries.len() > 256 {
        return Err("Sampler library needs between 1 and 256 presets".into());
    }
    entries.sort();
    for entry in &entries {
        if local_member("", entry)? != *entry {
            return Err("Nonlocal preset archive entry".into());
        }
    }
    Ok(entries)
}
fn renoise_members(zip: &zip::ZipArchive<File>) -> Result<Vec<(usize, String)>, String> {
    let mut members = Vec::new();
    for name in zip.file_names() {
        let Some(suffix) = name.strip_prefix("SampleData/Sample") else {
            continue;
        };
        let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || !suffix[digits..].starts_with([' ', '.']) {
            continue;
        }
        let index = suffix[..digits]
            .parse::<usize>()
            .map_err(|_| "Invalid Renoise sample index")?;
        let path = local_member("", name)?;
        members.push((index, path));
    }
    Ok(members)
}
fn local_member(manifest: &str, sample: &str) -> Result<String, String> {
    let normalized = sample.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains('\0') || normalized.len() > 4096 {
        return Err("Invalid archive-local sample path".into());
    }
    let mut parts: Vec<&str> = manifest
        .rsplit_once('/')
        .map(|(p, _)| p.split('/').collect())
        .unwrap_or_default();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or("Sample path escapes its archive")?;
            }
            part if part.contains(':') => {
                return Err(
                    "External sample references are not allowed in an instrument archive".into(),
                );
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err("Empty archive sample path".into());
    }
    Ok(parts.join("/"))
}
fn cancelled(token: &AtomicBool) -> Result<(), String> {
    if token.load(Ordering::Relaxed) {
        Err("Sample archive opening cancelled".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/multisample")
            .join(name)
    }
    #[test]
    fn packed_maps_resolve_members_without_extracting_samples() {
        for name in ["harbor.multisample", "harbor.dslibrary"] {
            let (document, archive) = read(&fixture(name), &AtomicBool::new(false)).unwrap();
            assert_eq!(document.instruments[0].regions.len(), 3);
            let region = &document.instruments[0].regions[1];
            assert_eq!(region.velocity, [64, 127]);
            let Some(EmbeddedSample::Archive { path }) = &region.embedded else {
                panic!()
            };
            assert_eq!(path, "Samples/bright.wav");
            let bytes = archive.member(path, &AtomicBool::new(false)).unwrap();
            assert_eq!(&bytes[..4], b"RIFF");
            assert!(archive.member(path, &AtomicBool::new(true)).is_err());
        }
    }
    #[test]
    fn selected_archive_member_survives_authorized_rename_and_rejects_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instrument.multisample");
        std::fs::copy(fixture("harbor.multisample"), &path).unwrap();
        let (_, mut archive) = read(&path, &AtomicBool::new(false)).unwrap();
        let source = FileSource::capture(&path).unwrap();
        let (destination, source) =
            super::super::super::edits::rename_file(&path, "renamed.multisample", &source).unwrap();
        archive.relocate(source);
        assert!(
            archive
                .member("Samples/soft.wav", &AtomicBool::new(false))
                .is_ok()
        );
        std::fs::write(&destination, b"replaced").unwrap();
        assert!(
            archive
                .member("Samples/soft.wav", &AtomicBool::new(false))
                .is_err()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn archive_paths_remain_local_and_plain_xml_is_not_a_library() {
        assert_eq!(
            local_member("Presets/Keys.dspreset", "../Samples/A.wav").unwrap(),
            "Samples/A.wav"
        );
        for path in [
            "../../outside.wav",
            "/etc/passwd",
            "https://example.org/x.wav",
            "C:\\Samples\\A.wav",
        ] {
            assert!(local_member("Presets/Keys.dspreset", path).is_err());
        }
        assert!(read(Path::new("/dev/zero"), &AtomicBool::new(false)).is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("generic.dslibrary");
        std::fs::write(&path, b"<root><preset/></root>").unwrap();
        assert!(read(&path, &AtomicBool::new(false)).is_err());
    }
}
