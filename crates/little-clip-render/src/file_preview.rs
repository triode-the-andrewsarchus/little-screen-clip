//! File previews prepared by the frame worker before card rasterization.
use little_clip_core::{
    ClipItem, ClipPayload, ImageThumbnail,
    brushes::BrushLibrary,
    file_format::Format,
    file_inventory::Identity,
    file_source::FileSource,
    geography::{Document, places, trajectory::Trajectory},
    midi::MidiData,
    minecraft,
    spreadsheet::{self, SpreadsheetSheet},
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 8;
const RETRY_AFTER: Duration = Duration::from_secs(10);

pub(crate) struct Page {
    pub width: u32,
    pub height: u32,
    pub thumbnail: ImageThumbnail,
}

pub(crate) enum Preview {
    Page(Page),
    Spreadsheet {
        title: String,
        sheet: SpreadsheetSheet,
    },
    Midi(MidiData),
    Scope(little_clip_core::engineering::waveforms::Overview),
    Touchstone(little_clip_core::engineering::touchstone::Summary),
    Logic(little_clip_core::engineering::logic_preview::Overview),
    Packets(little_clip_core::engineering::packets::Summary),
    Can(little_clip_core::engineering::can::Summary),
    Pulses(little_clip_core::engineering::pulses::Capture),
    Tags(little_clip_core::engineering::tags::Document),
    Radio(little_clip_core::engineering::radio::Summary),
    Brush(BrushLibrary),
    Map(super::map_preview::Map),
    Text {
        contents: String,
        extension: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pdf,
    Office,
    Spreadsheet,
    Midi,
    Scope,
    Touchstone,
    Logic,
    Packets,
    Can,
    Pulses,
    Tags,
    Radio,
    EngineeringCandidate,
    Brush,
    Embroidery,
    Geography,
    Trajectory,
    Text,
}

struct Entry {
    source: FileSource,
    kind: Kind,
    extension: Option<String>,
    preview: Option<Arc<Preview>>,
    prepared: Instant,
}

type CachedPreview = Option<Arc<Preview>>;

struct MinecraftEntry {
    path: PathBuf,
    revision: Identity,
    prepared: Instant,
    preview: CachedPreview,
}

#[derive(Default)]
pub(crate) struct Cache {
    files: VecDeque<Entry>,
    locations: VecDeque<(String, CachedPreview)>,
    minecraft: VecDeque<MinecraftEntry>,
}

impl Cache {
    fn has_minecraft_map(&self, path: &Path) -> bool {
        self.minecraft
            .iter()
            .any(|entry| entry.path == path && entry.preview.is_some())
    }
}

impl crate::ScreenRenderer {
    /// Reports whether the last card render prepared a map for this path.
    pub fn cached_map_for_file(&self, path: &Path) -> bool {
        let cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.files.iter().any(|entry| {
            entry.source.path() == path && matches!(entry.preview.as_deref(), Some(Preview::Map(_)))
        }) || cache.has_minecraft_map(path)
    }

    /// Reports whether a Minecraft save or region has a prepared preview.
    pub fn cached_minecraft_map_for_file(&self, path: &Path) -> bool {
        self.file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_minecraft_map(path)
    }

    pub(crate) fn file_preview(&self, item: &ClipItem) -> Option<Arc<Preview>> {
        self.path_preview(item).or_else(|| match &item.payload {
            ClipPayload::Text(text) if text.trim().len() <= 128 => {
                self.location_preview(text.trim())
            }
            _ => None,
        })
    }

    fn path_preview(&self, item: &ClipItem) -> Option<Arc<Preview>> {
        let (path, documents) = match &item.payload {
            ClipPayload::FileBatch(files) if files.len() == 1 => (files[0].as_path(), true),
            ClipPayload::Directory(path) => (path.as_path(), true),
            ClipPayload::Text(text) => (Path::new(text.trim()), false),
            _ => return None,
        };
        if minecraft::is_map_file(path) || (documents && path.is_dir()) {
            return self.minecraft_preview(path);
        }
        let extension = little_clip_core::diagram::Format::source_extension_hint(path);
        let kind = file_kind(path, documents, extension)?;
        let source = FileSource::capture(path).ok()?;
        {
            let mut cache = self
                .file_previews
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = cache.files.iter().position(|entry| {
                entry.source == source
                    && entry.kind == kind
                    && entry.extension.as_deref() == extension
            }) {
                let entry = cache.files.remove(index).expect("cache entry exists");
                if entry.preview.is_some() || entry.prepared.elapsed() < RETRY_AFTER {
                    let preview = entry.preview.clone();
                    cache.files.push_back(entry);
                    return preview;
                }
            }
        }
        let preview = self.load_path_preview(path, kind, extension).map(Arc::new);
        if FileSource::capture(path).ok().as_ref() != Some(&source) {
            return None;
        }
        let mut cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache
            .files
            .retain(|entry| entry.source.path() != source.path());
        if cache.files.len() >= MAX_ENTRIES {
            cache.files.pop_front();
        }
        cache.files.push_back(Entry {
            source,
            kind,
            extension: extension.map(str::to_owned),
            preview: preview.clone(),
            prepared: Instant::now(),
        });
        preview
    }

    fn load_path_preview(
        &self,
        path: &Path,
        kind: Kind,
        extension: Option<&str>,
    ) -> Option<Preview> {
        if kind == Kind::Geography {
            return self.earth_preview(&Document::from_path(path).ok()?);
        }
        match little_clip_core::file_format::recognize_engineering(path, &AtomicBool::new(false)) {
            Some(Format::Oscilloscope) => return load(path, Kind::Scope, extension),
            Some(Format::Touchstone) => return load(path, Kind::Touchstone, extension),
            Some(Format::LogicCapture) => return load(path, Kind::Logic, extension),
            Some(Format::PacketCapture) => return load(path, Kind::Packets, extension),
            Some(Format::CanCapture) => return load(path, Kind::Can, extension),
            Some(Format::RadioProgramming) => return load(path, Kind::Radio, extension),
            Some(Format::PulseSignals) => return load(path, Kind::Pulses, extension),
            Some(Format::TagCapture | Format::OneWire) => return load(path, Kind::Tags, extension),
            _ => {}
        }
        if let Ok(Some(trajectory)) = Trajectory::read_if_identified(path, &AtomicBool::new(false))
        {
            return crate::render_trajectory_thumbnail(&trajectory).map(|thumbnail| {
                Preview::Map(super::map_preview::Map {
                    thumbnail,
                    label: Some(trajectory.name),
                })
            });
        }
        load(path, kind, extension)
    }

    fn earth_preview(&self, document: &Document) -> Option<Preview> {
        let texture = self
            .map_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .earth(&self.config.geo_assets_dir());
        super::map_preview::Map::earth(self, document, texture.as_ref()).map(Preview::Map)
    }

    fn minecraft_preview(&self, path: &Path) -> CachedPreview {
        let revision = minecraft::preview_revision(path)?;
        let mut cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = cache.minecraft.iter().position(|entry| entry.path == path)
            && let Some(entry) = cache.minecraft.remove(index)
            && entry.revision == revision
            && entry.prepared.elapsed() < RETRY_AFTER
        {
            let preview = entry.preview.clone();
            cache.minecraft.push_back(entry);
            return preview;
        }
        drop(cache);

        let cancel = AtomicBool::new(false);
        let preview = (|| {
            let world = minecraft::World::open(path, &cancel).ok()?;
            let thumbnail = world.thumbnail(&cancel).ok()?;
            Some(Arc::new(Preview::Map(super::map_preview::Map {
                thumbnail,
                label: Some("Minecraft".into()),
            })))
        })();
        if minecraft::preview_revision(path) != Some(revision) {
            return None;
        }
        let mut cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.minecraft.retain(|entry| entry.path != path);
        if cache.minecraft.len() >= MAX_ENTRIES {
            cache.minecraft.pop_front();
        }
        cache.minecraft.push_back(MinecraftEntry {
            path: path.to_owned(),
            revision,
            prepared: Instant::now(),
            preview: preview.clone(),
        });
        preview
    }

    fn location_preview(&self, text: &str) -> CachedPreview {
        {
            let mut cache = self
                .file_previews
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = cache.locations.iter().position(|(key, _)| key == text) {
                let entry = cache.locations.remove(index).expect("cache entry exists");
                let result = entry.1.clone();
                cache.locations.push_back(entry);
                return result;
            }
        }
        let result = places::resolve(text)
            .and_then(|location| Document::from_location(&location).ok())
            .and_then(|document| self.earth_preview(&document))
            .map(Arc::new);
        let mut cache = self
            .file_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cache.locations.len() >= MAX_ENTRIES {
            cache.locations.pop_front();
        }
        cache.locations.push_back((text.to_owned(), result.clone()));
        result
    }
}

fn file_kind(path: &Path, documents: bool, extension: Option<&str>) -> Option<Kind> {
    let kind = if Format::for_path(path) == Some(Format::Geography) {
        Kind::Geography
    } else if matches!(
        Format::for_path(path),
        Some(Format::Oscilloscope | Format::PowerRecording)
    ) {
        Kind::Scope
    } else if Format::for_path(path) == Some(Format::Touchstone) {
        Kind::Touchstone
    } else if Format::for_path(path) == Some(Format::LogicCapture) {
        Kind::Logic
    } else if Format::for_path(path) == Some(Format::PacketCapture) {
        Kind::Packets
    } else if Format::for_path(path) == Some(Format::PulseSignals) {
        Kind::Pulses
    } else if Format::for_path(path) == Some(Format::RadioProgramming) {
        Kind::Radio
    } else if Format::for_path(path) == Some(Format::Embroidery) {
        Kind::Embroidery
    } else if spreadsheet::is_spreadsheet_file(path) {
        Kind::Spreadsheet
    } else if extension.is_some_and(|value| {
        value.eq_ignore_ascii_case("mid") || value.eq_ignore_ascii_case("midi")
    }) {
        Kind::Midi
    } else if little_clip_core::brushes::is_brush_file(path) {
        Kind::Brush
    } else if documents && little_clip_core::document::is_office_document(path) {
        Kind::Office
    } else if documents && little_clip_core::pdf::is_pdf_path(path) {
        Kind::Pdf
    } else if little_clip_core::syntax::is_code_file(path) {
        Kind::Text
    } else if Trajectory::is_candidate(path) {
        Kind::Trajectory
    } else if little_clip_core::file_format::ambiguous_engineering_path(path) {
        Kind::EngineeringCandidate
    } else {
        return None;
    };
    Some(kind)
}

fn load(path: &Path, kind: Kind, extension: Option<&str>) -> Option<Preview> {
    match kind {
        Kind::Geography | Kind::Trajectory | Kind::EngineeringCandidate => None,
        Kind::Scope => {
            let cancel = AtomicBool::new(false);
            let recording = little_clip_core::engineering::waveforms::Recording::open(
                path,
                Default::default(),
                &cancel,
            )
            .ok()?;
            recording.overview(&cancel).ok().map(Preview::Scope)
        }
        Kind::Touchstone => {
            use little_clip_core::{engineering::touchstone, file_source::ReadCheck};
            let cancel = AtomicBool::new(false);
            let (file, source) = ReadCheck::default().open(path, None, &cancel).ok()?;
            let sweep = touchstone::summary(&file, path, 800, &cancel).ok()?;
            source.check_file(&file).ok()?;
            Some(Preview::Touchstone(sweep))
        }
        Kind::Logic => {
            let cancel = AtomicBool::new(false);
            let recording =
                little_clip_core::engineering::logic_preview::Recording::open(path, &cancel)
                    .ok()?;
            recording.overview(&cancel).ok().map(Preview::Logic)
        }
        Kind::Packets => {
            use little_clip_core::{engineering::packets, file_source::ReadCheck};
            let cancel = AtomicBool::new(false);
            let (file, source) = ReadCheck::default().open(path, None, &cancel).ok()?;
            let summary = packets::summary(std::io::BufReader::new(&file), &cancel).ok()?;
            source.check_file(&file).ok()?;
            Some(Preview::Packets(summary))
        }
        Kind::Can => {
            use little_clip_core::{engineering::can, file_source::ReadCheck};
            let cancel = AtomicBool::new(false);
            let (file, source) = ReadCheck::default().open(path, None, &cancel).ok()?;
            let capture = can::Capture::open(file.try_clone().ok()?).ok()?;
            let summary = capture.preview(&cancel).ok()?;
            source.check_file(&file).ok()?;
            Some(Preview::Can(summary))
        }
        Kind::Pulses => {
            little_clip_core::engineering::pulses::Capture::preview(path, &AtomicBool::new(false))
                .ok()
                .map(Preview::Pulses)
        }
        Kind::Tags => {
            use little_clip_core::{engineering::tags, file_source::ReadCheck};
            let cancel = AtomicBool::new(false);
            let (file, source) = ReadCheck::default().open(path, None, &cancel).ok()?;
            let tags = tags::preview(path, std::io::BufReader::new(&file), &cancel).ok()?;
            source.check_file(&file).ok()?;
            Some(Preview::Tags(tags))
        }
        Kind::Radio => little_clip_core::engineering::radio::preview(path, &AtomicBool::new(false))
            .ok()
            .map(Preview::Radio),
        Kind::Embroidery => {
            let cancelled = AtomicBool::new(false);
            let design = little_clip_core::embroidery::Design::read(path, &cancelled).ok()?;
            let thumbnail = design.thumbnail([320, 220], &cancelled).ok()?;
            Some(Preview::Page(Page {
                // These dimensions describe source pixels, which stitch files lack.
                width: 0,
                height: 0,
                thumbnail,
            }))
        }
        Kind::Pdf | Kind::Office => load_page(path, kind == Kind::Office).map(Preview::Page),
        Kind::Spreadsheet => {
            let data = spreadsheet::load_spreadsheet_preview(
                path,
                9,
                10,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .ok()?;
            let sheet = data.sheets.into_iter().next()?;
            Some(Preview::Spreadsheet {
                title: data.title,
                sheet,
            })
        }
        Kind::Midi => {
            let mut score = MidiData::parse_file(path).ok()?;
            // Keep notes across the full timeline while bounding dense scores.
            let stride = score.notes.len().div_ceil(16_384).max(1);
            if stride > 1 {
                score.notes = score.notes.into_iter().step_by(stride).collect();
            }
            Some(Preview::Midi(score))
        }
        Kind::Brush => {
            let cancelled = std::sync::atomic::AtomicBool::new(false);
            little_clip_core::brushes::load_brush_library(path, 6, &cancelled)
                .ok()
                .map(Preview::Brush)
        }
        Kind::Text => {
            let contents = little_clip_core::kernel_interface::read_content_to_string_bounded(
                path,
                if extension == Some("excalidraw") {
                    little_clip_core::diagram::MAX_DRAWING_SOURCE_BYTES
                } else {
                    1024 * 1024
                },
            )
            .ok()?;
            Some(Preview::Text {
                contents,
                extension: extension.map(str::to_owned),
            })
        }
    }
}

fn load_page(path: &Path, office: bool) -> Option<Page> {
    let converted;
    let pdf = if office {
        converted = little_clip_core::document::ensure_rendered_pdf(path).ok()?;
        converted.as_path()
    } else {
        path
    };
    let (png, width, height) =
        little_clip_core::pdf::render_pdf_page_thumbnail(pdf, 1, 800).ok()?;
    let thumbnail = ImageThumbnail::generate(&png, 320, 220)?;
    Some(Page {
        width,
        height,
        thumbnail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minecraft_cards_put_terrain_in_the_little_screen_jpeg() {
        let renderer = crate::ScreenRenderer::new(Default::default()).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/minecraft");
        for item in [
            ClipItem::from_directory(fixture.clone()),
            ClipItem::from_files(vec![fixture.join("level.dat")]),
            ClipItem::from_files(vec![fixture.join("region/r.-1.2.mca")]),
        ] {
            let path = item.file_path().unwrap();
            let mut inventory = little_clip_core::SlotManager::new();
            inventory.push_left(item);
            let frame = renderer.render_frame(&inventory).unwrap();
            let image = image::load_from_memory(&frame).unwrap().to_rgb8();
            let image = if renderer.config.display.rotate_180 {
                image::imageops::rotate180(&image)
            } else {
                image
            };
            assert_eq!(
                image.dimensions(),
                (crate::SCREEN_WIDTH, crate::SCREEN_HEIGHT)
            );
            let slot = crate::compute_slot_layout(3)
                .into_iter()
                .find(|slot| slot.slot_id == little_clip_core::SlotId::LeftHand)
                .unwrap();
            let grass_pixels = image
                .enumerate_pixels()
                .filter(|(x, _, image::Rgb([r, g, b]))| {
                    (slot.x..slot.x + slot.width).contains(x)
                        && *g > r.saturating_add(15)
                        && *g > b.saturating_add(30)
                })
                .count();
            assert!(
                grass_pixels > 1_000,
                "Missing terrain in preview for {path:?}"
            );
            assert!(renderer.cached_map_for_file(&path));
        }
    }

    #[test]
    fn lincoln_clipboard_map_contains_a_pin_and_an_inset_city_label() {
        let root = tempfile::tempdir().unwrap();
        let mut config = little_clip_core::AppConfig::default();
        config.geography.assets_dir = Some(root.path().to_owned());
        let renderer = crate::ScreenRenderer::new(config).unwrap();
        let location = ClipItem::from_text("40.813599 -96.702606");
        let preview = renderer.file_preview(&location).unwrap();
        let Preview::Map(map) = preview.as_ref() else {
            panic!("Expected a map preview")
        };
        let pixels = &map.thumbnail.premultiplied_rgba;
        assert!(
            pixels
                .chunks_exact(4)
                .any(|rgba| rgba == [115, 241, 211, 255])
        );
        assert!(
            pixels
                .chunks_exact(4)
                .any(|rgba| rgba == [255, 255, 255, 255])
        );
    }

    #[test]
    fn maps_are_prepared_for_geography_and_standalone_clipboard_locations() {
        let renderer = crate::ScreenRenderer::fallback(Default::default());
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/harbor.gpx");
        let item = ClipItem::from_files(vec![fixture]);
        let preview = renderer.file_preview(&item).unwrap();
        let Preview::Map(map) = preview.as_ref() else {
            panic!("Expected a map preview")
        };
        assert!(
            map.thumbnail
                .premultiplied_rgba
                .chunks_exact(4)
                .any(|rgba| rgba == [115, 241, 211, 255])
        );

        for text in ["41.8781, -87.6298", "Chicago"] {
            let item = ClipItem::from_text(text);
            assert!(matches!(
                renderer.file_preview(&item).as_deref(),
                Some(Preview::Map(_))
            ));
            assert_eq!(item.to_clipboard_text().as_deref(), Some(text));
        }
        let sentence = ClipItem::from_text("The trip starts in Chicago.");
        assert!(renderer.file_preview(&sentence).is_none());
    }
}
