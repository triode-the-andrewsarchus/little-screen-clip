//! Read-only member snapshots: archive origins never become filesystem paths.
use super::inventory::{self, Child, Inventory};
use crate::file_manager::{
    content::Content,
    media::PreviewData,
    shelf::preview::{Previews as Cache, Ready, Source},
};
use crate::{
    file_source::{FileSource, ReadCheck},
    icons::Icon,
    shell::StudioShell,
};
use little_clip_core::{BackgroundJobs, palette::AppPalette};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{LazyLock, atomic::AtomicBool},
    time::Duration,
};

pub(in crate::file_manager) static JOBS: LazyLock<BackgroundJobs> =
    LazyLock::new(|| BackgroundJobs::new(2, 4));

#[derive(Clone)]
pub(in crate::file_manager) struct Request {
    source: FileSource,
    ordinal: usize,
    edge: u32,
    check: ReadCheck,
}
pub(in crate::file_manager) fn preview(request: &Request, cancel: &AtomicBool) -> PreviewData {
    match request.check.read(cancel, || {
        inventory::read_archive(&request.source, Some(request.ordinal), cancel)
    }) {
        Ok(mut inventory) => {
            let name = inventory
                .children
                .iter()
                .find_map(|child| match child {
                    Child::Member { ordinal, name, .. } if *ordinal == request.ordinal => {
                        Some(name.clone())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "Archive member".into());
            let bytes = inventory.payload.take().unwrap_or_default();
            let mut data = member_preview(bytes, request.edge);
            data.title = name;
            data
        }
        Err(error) => PreviewData::new("Archive member".into(), error, Icon::Text),
    }
}
fn member_preview(bytes: Vec<u8>, edge: u32) -> PreviewData {
    let mime = image::guess_format(&bytes)
        .ok()
        .map(|format| format.to_mime_type());
    if let Some(mime) = mime {
        return match Content::capture(bytes, mime) {
            Ok(content) => {
                let mut data =
                    PreviewData::new(content.default_name(), content.mime.clone(), Icon::Image);
                let edge = edge.clamp(128, 1024);
                data.image = crate::file_manager::shelf::preview::sized_thumbnail(
                    &content.bytes,
                    Some(&content.mime),
                    edge,
                    edge,
                );
                data
            }
            Err(error) => PreviewData::new("Image".into(), error, Icon::Image),
        };
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return PreviewData::new(
            "Member".into(),
            "Binary member · metadata only".into(),
            Icon::Text,
        );
    };
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return PreviewData::new(
            "Member".into(),
            "Binary member · metadata only".into(),
            Icon::Text,
        );
    }
    let clipped = bytes.len() > 64 * 1024;
    let mut end = bytes.len().min(64 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut data = PreviewData::new(
        "Text".into(),
        if clipped {
            "Read-only text · clipped to 64 KiB"
        } else {
            "Read-only text"
        }
        .into(),
        Icon::Text,
    );
    data.text = text[..end].to_owned();
    data
}

#[derive(Default)]
pub(super) struct Previews {
    edge: u32,
    cache: Cache,
    ids: BTreeMap<(PathBuf, usize), (Vec<u8>, uuid::Uuid)>,
    keep: BTreeSet<uuid::Uuid>,
}
impl Previews {
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.keep.clear();
        self.cache.poll(ctx);
        if self.cache.has_pending() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }
    pub fn finish(&mut self) {
        self.cache.retain(&self.keep);
        self.ids.retain(|_, (_, id)| self.keep.contains(id));
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.ids.clear();
        self.keep.clear();
    }
    pub fn request(
        &mut self,
        source: &FileSource,
        ordinal: usize,
        check: ReadCheck,
    ) -> Option<&Ready> {
        let key = (source.path().to_path_buf(), ordinal);
        if self.ids.len() >= 128 && !self.ids.contains_key(&key) {
            return None;
        }
        let revision = source.revision_key();
        let slot = self
            .ids
            .entry(key)
            .or_insert_with(|| (revision.clone(), uuid::Uuid::new_v4()));
        if slot.0 != revision {
            self.cache.invalidate(slot.1);
            *slot = (revision, uuid::Uuid::new_v4());
        }
        let id = slot.1;
        self.keep.insert(id);
        if self.cache.needs(id) {
            self.cache.request(
                id,
                Source::ArchiveMember(Request {
                    source: source.clone(),
                    ordinal,
                    edge: self.edge.max(128),
                    check,
                }),
            );
        }
        self.cache.get(id)
    }
}

pub(in crate::file_manager) struct Viewer {
    source: FileSource,
    inventory: Inventory,
    selected: Option<usize>,
    folder: String,
    previews: Previews,
    check: ReadCheck,
}
impl Viewer {
    pub fn load(path: &std::path::Path, cancel: &AtomicBool) -> Result<Self, String> {
        let source = FileSource::capture(path)?;
        let inventory = inventory::read_archive(&source, None, cancel)?;
        Ok(Self {
            source,
            inventory,
            selected: None,
            folder: String::new(),
            previews: Previews {
                edge: 1024,
                ..Previews::default()
            },
            check: ReadCheck::default(),
        })
    }
    pub fn file_source(&self) -> FileSource {
        self.source.clone()
    }
    pub fn set_check(&mut self, check: ReadCheck) {
        self.check = check;
    }
    pub fn relocate_file(&mut self, source: FileSource) {
        self.source = source;
        self.previews.clear();
    }
    pub fn select(&mut self, ordinal: usize) {
        if let Some(Child::Member {
            name, directory, ..
        }) = self
            .inventory
            .children
            .iter()
            .find(|child| matches!(child, Child::Member { ordinal: id, .. } if *id == ordinal))
        {
            self.folder = if *directory {
                name.clone()
            } else {
                name.rsplit_once('/')
                    .map_or(String::new(), |(parent, _)| parent.into())
            };
            self.selected = (!directory).then_some(ordinal);
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        StudioShell::new("Archive", Icon::Folder, palette, close).render_header(ui, |_| {});
        self.previews.begin(ui.ctx());
        ui.horizontal_wrapped(|ui| {
            if ui.button("Archive root").clicked() {
                self.folder.clear();
                self.selected = None;
            }
            if !self.folder.is_empty() && ui.button("Up").clicked() {
                self.folder = self
                    .folder
                    .rsplit_once('/')
                    .map_or(String::new(), |(parent, _)| parent.into());
                self.selected = None;
            }
            ui.label(if self.folder.is_empty() {
                "/"
            } else {
                &self.folder
            });
            ui.weak(format!(
                "{}{} members · read-only",
                self.inventory.observed,
                if self.inventory.verified { "" } else { "+" }
            ));
        });
        if !self.inventory.verified {
            ui.label(
                self.inventory
                    .notice
                    .as_deref()
                    .unwrap_or("Archive inventory limit reached"),
            );
        }
        let rows = directory_rows(&self.inventory, &self.folder);
        egui::SidePanel::left("archive-members")
            .default_width(260.0)
            .width_range(140.0..=500.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show_rows(ui, 26.0, rows.len(), |ui, range| {
                    for (name, directory, ordinal) in &rows[range] {
                        let label = format!("{} {}", if *directory { "▸" } else { "" }, name);
                        if ui
                            .selectable_label(!directory && *ordinal == self.selected, label)
                            .clicked()
                        {
                            if *directory {
                                self.folder = if self.folder.is_empty() {
                                    name.clone()
                                } else {
                                    format!("{}/{name}", self.folder)
                                };
                                self.selected = None;
                            } else {
                                self.selected = *ordinal;
                            }
                        }
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(8.0))
            .show_inside(ui, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                self.selected_ui(ui);
            });
        self.previews.finish();
    }
    fn selected_ui(&mut self, ui: &mut egui::Ui) {
        let Some(ordinal) = self.selected else {
            ui.label("Select a member to preview");
            return;
        };
        let Some(Child::Member { name, size, .. }) = self
            .inventory
            .children
            .iter()
            .find(|child| matches!(child, Child::Member { ordinal: id, .. } if *id == ordinal))
        else {
            return;
        };
        ui.add(egui::Label::new(name).truncate(true))
            .on_hover_text(name);
        ui.weak(format!(
            "{} · archive member · read-only",
            crate::file_manager::filesystem::format_size(*size)
        ));
        let Some(ready) = self
            .previews
            .request(&self.source, ordinal, self.check.clone())
        else {
            ui.spinner();
            return;
        };
        ui.label(&ready.data.detail);
        if let Some(texture) = &ready.texture {
            let available = ui.available_size();
            let scale = (available.x / texture.size_vec2().x)
                .min(available.y / texture.size_vec2().y)
                .max(0.0);
            ui.image((texture.id(), texture.size_vec2() * scale));
        } else {
            let lines: Vec<_> = ready.data.text.lines().collect();
            egui::ScrollArea::both().show_rows(ui, 18.0, lines.len(), |ui, range| {
                for line in &lines[range] {
                    ui.add(egui::Label::new(egui::RichText::new(*line).monospace()).wrap(false));
                }
            });
        }
    }
}
fn directory_rows(inventory: &Inventory, folder: &str) -> Vec<(String, bool, Option<usize>)> {
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut rows = BTreeMap::new();
    for child in &inventory.children {
        let Child::Member {
            name,
            directory,
            ordinal,
            ..
        } = child
        else {
            continue;
        };
        let Some(relative) = name.strip_prefix(&prefix).filter(|name| !name.is_empty()) else {
            continue;
        };
        let (name, nested) = relative
            .split_once('/')
            .map_or((relative, false), |(first, _)| (first, true));
        let directory = nested || *directory;
        rows.entry((!directory, name.to_owned())).or_insert((
            name.to_owned(),
            directory,
            (!directory).then_some(*ordinal),
        ));
    }
    rows.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Cursor, Write},
        sync::atomic::Ordering,
    };
    fn zip(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    fn source(path: &std::path::Path) -> FileSource {
        FileSource::capture(path).unwrap()
    }
    fn request(source: FileSource, ordinal: usize) -> Request {
        Request {
            source,
            ordinal,
            edge: 128,
            check: ReadCheck::default(),
        }
    }
    #[test]
    fn archive_member_ordinals_survive_sorting_and_full_inventory_exceeds_inline_cards() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("members.zip");
        let names: Vec<_> = (0..80)
            .rev()
            .map(|i| format!("folder/note-{i:02}.txt"))
            .collect();
        let records: Vec<_> = names
            .iter()
            .map(|name| (name.as_str(), name.as_bytes()))
            .collect();
        zip(&path, &records);
        let held = source(&path);
        let cancel = AtomicBool::new(false);
        let listing = inventory::read_archive(&held, None, &cancel).unwrap();
        assert!(listing.verified);
        assert_eq!(listing.children.len(), 80);
        assert_eq!(
            directory_rows(&listing, ""),
            [("folder".into(), true, None)]
        );
        assert_eq!(directory_rows(&listing, "folder").len(), 80);
        let data = preview(&request(held, 0), &cancel);
        assert_eq!(data.title, "folder/note-79.txt");
        assert_eq!(data.text, "folder/note-79.txt");
        assert!(!root.path().join("folder").exists());
    }
    #[test]
    fn image_members_use_the_shared_decoder_and_binary_members_remain_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("images.zip");
        let mut png = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1600, 900, image::Rgba([20, 100, 220, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        zip(
            &path,
            &[("blue.png", png.get_ref()), ("binary.dat", &[0, 1, 255])],
        );
        let held = source(&path);
        let cancel = AtomicBool::new(false);
        let small = preview(&request(held.clone(), 0), &cancel).image.unwrap();
        assert!(small.width <= 128 && small.height <= 128);
        let mut expanded = request(held.clone(), 0);
        expanded.edge = 1024;
        let large = preview(&expanded, &cancel).image.unwrap();
        assert!(large.width > 128 && large.width <= 1024 && large.height <= 1024);
        let data = preview(&request(held, 1), &cancel);
        assert!(data.detail.contains("metadata only"));
        assert!(data.text.is_empty());
    }
    #[test]
    fn stale_source_and_read_guard_reject_member_reads() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.zip");
        zip(&path, &[("note.txt", b"first")]);
        let held = source(&path);
        let cancel = AtomicBool::new(false);
        let mut denied = request(held.clone(), 0);
        denied.check = ReadCheck::new(|_| Err("source scope rejected".into()));
        assert_eq!(preview(&denied, &cancel).detail, "source scope rejected");
        std::fs::rename(&path, root.path().join("original.zip")).unwrap();
        zip(&path, &[("note.txt", b"replacement")]);
        let data = preview(&request(held, 0), &cancel);
        assert!(data.text.is_empty());
        assert!(data.detail.contains("changed") || data.detail.contains("Source"));
        cancel.store(true, Ordering::Relaxed);
        assert!(
            inventory::read_archive(&source(&path), None, &cancel)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn gzip_member_preview_requires_valid_trailer_and_rejects_later_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("notes.tgz");
        let mut archive = tar::Builder::new(Vec::new());
        for bytes in [b"first".as_slice(), b"second".as_slice()] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            archive.append_data(&mut header, "same.txt", bytes).unwrap();
        }
        let mut compressed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        compressed
            .write_all(&archive.into_inner().unwrap())
            .unwrap();
        let bytes = compressed.finish().unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let data = preview(&request(source(&path), 0), &AtomicBool::new(false));
        assert!(data.detail.contains("Duplicate"));
        assert!(data.text.is_empty());
        // Independently validate a truncated trailer, rather than truncating a
        // stream whose checksum was already corrupt.
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(5);
        header.set_cksum();
        tar.append_data(&mut header, "note.txt", b"hello".as_slice())
            .unwrap();
        encoder.write_all(&tar.into_inner().unwrap()).unwrap();
        let good = encoder.finish().unwrap();
        std::fs::write(&path, &good).unwrap();
        assert_eq!(
            preview(&request(source(&path), 0), &AtomicBool::new(false)).text,
            "hello"
        );
        std::fs::write(&path, &good[..good.len() - 4]).unwrap();
        let data = preview(&request(source(&path), 0), &AtomicBool::new(false));
        assert!(data.text.is_empty());
        assert!(!data.detail.starts_with("Read-only"));
    }
    #[test]
    fn utf8_text_is_clipped_at_a_character_boundary_and_large_members_are_rejected() {
        let data = member_preview("🎹".repeat(20000).into_bytes(), 128);
        assert_eq!(data.text.len(), 64 * 1024);
        assert!(data.detail.contains("clipped"));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("large.zip");
        zip(&path, &[("huge.txt", &vec![b'x'; 8 * 1024 * 1024 + 1])]);
        let data = preview(&request(source(&path), 0), &AtomicBool::new(false));
        assert!(data.detail.contains("8 MiB"));
        assert!(data.text.is_empty());
    }
    #[test]
    fn plain_tar_member_preview_survives_archive_rename_with_new_source_identity() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("before.tar");
        let mut builder = tar::Builder::new(std::fs::File::create(&path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(5);
        header.set_cksum();
        builder
            .append_data(&mut header, "nested/note.txt", b"hello".as_slice())
            .unwrap();
        builder.into_inner().unwrap().flush().unwrap();
        let cancel = AtomicBool::new(false);
        let mut viewer = Viewer::load(&path, &cancel).unwrap();
        viewer.select(0);
        assert_eq!(viewer.folder, "nested");
        let (new_path, source) =
            crate::file_manager::edits::rename_file(&path, "after.tar", &viewer.source).unwrap();
        viewer.relocate_file(source);
        assert_eq!(viewer.previews.edge, 1024);
        let data = preview(
            &Request {
                source: viewer.source.clone(),
                ordinal: viewer.selected.unwrap(),
                edge: 128,
                check: ReadCheck::default(),
            },
            &cancel,
        );
        assert_eq!(data.text, "hello");
        assert_eq!(data.title, "nested/note.txt");
        assert!(!path.exists() && new_path.exists());
        assert!(!root.path().join("nested").exists());
    }
}
