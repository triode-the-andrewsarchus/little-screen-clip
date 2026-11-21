//! In-memory browser ordering. No file probing or preview readiness dependencies.
pub(super) use little_clip_core::natural_sort::natural_cmp;
use std::cmp::Ordering;

use super::{
    FileManager,
    filesystem::{Directory, Entry},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum Column {
    Type,
    #[default]
    Name,
    Size,
    Modified,
    Created,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Sort {
    pub column: Column,
    pub descending: bool,
}

impl Sort {
    pub fn apply(self, directory: &Directory, visible: &mut Vec<usize>) {
        // Normalize once per entry, rather than allocating inside O(n log n)
        // comparisons. Type categories reuse the browser's path classifiers.
        let mut keys: Vec<_> = visible
            .iter()
            .map(|index| {
                let entry = &directory.entries[*index];
                let kind = (self.column == Column::Type).then(|| family(entry));
                (*index, entry, entry.name.to_lowercase(), kind)
            })
            .collect();
        keys.sort_by(|a, b| {
            let primary = match self.column {
                Column::Name => directed(natural_cmp(&a.2, &b.2), self.descending),
                Column::Type => directed(a.3.cmp(&b.3), self.descending),
                Column::Size => available(
                    (!a.1.directory && a.1.warning.is_none()).then_some(a.1.size_bytes),
                    (!b.1.directory && b.1.warning.is_none()).then_some(b.1.size_bytes),
                    self.descending,
                ),
                Column::Created => available(a.1.created_time, b.1.created_time, self.descending),
                Column::Modified => {
                    available(a.1.modified_time, b.1.modified_time, self.descending)
                }
            };
            b.1.directory
                .cmp(&a.1.directory)
                .then(primary)
                .then_with(|| natural_cmp(&a.2, &b.2))
                .then_with(|| a.1.path.cmp(&b.1.path))
        });
        *visible = keys.into_iter().map(|(index, ..)| index).collect();
    }
}

fn directed(order: Ordering, descending: bool) -> Ordering {
    if descending { order.reverse() } else { order }
}

pub(super) fn available<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => directed(a.cmp(&b), descending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

// Families deliberately group related formats; natural names order all members
// without splitting an image or audio family back into extension groups.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Category {
    Folder,
    Audio,
    VcvRack,
    Image,
    Brushes,
    Video,
    Scene,
    Font,
    Geography,
    Document,
    Text,
    Archive,
    Binary,
    Other,
    System,
}

pub(super) fn family(entry: &Entry) -> Category {
    if entry.directory {
        Category::Folder
    } else if entry.system.is_some() || !entry.regular_file {
        Category::System
    } else if entry.is_brush() {
        Category::Brushes
    } else {
        format_family(entry)
    }
}

fn format_family(entry: &Entry) -> Category {
    if entry.is_vcv_patch() {
        Category::VcvRack
    } else if little_clip_core::audio::is_audio_path(&entry.path)
        || super::midi_strip::is_midi(&entry.path)
        || entry.is_multisample()
    {
        Category::Audio
    } else if entry.is_photo() {
        Category::Image
    } else if entry.is_video() {
        Category::Video
    } else if entry.is_scene() || entry.is_blend() || entry.is_material() {
        Category::Scene
    } else if super::font_strip::known(&entry.path) {
        Category::Font
    } else {
        match extension_category(entry) {
            Category::Other if super::text_strip::known_text(&entry.path) => Category::Text,
            Category::Other if super::binary_strip::known(&entry.path) => Category::Binary,
            category => category,
        }
    }
}

fn extension_category(entry: &Entry) -> Category {
    if entry.is_geography() {
        return Category::Geography;
    }
    if little_clip_core::notebook::is_candidate_path(&entry.path) {
        return Category::Document;
    }
    let extension = entry
        .path
        .extension()
        .map(|v| v.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "pdf" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
        | "epub" | "cbz" | "sfz" | "sf2" | "nki" | "gcode" | "sqlite" | "sqlite3" | "db"
        | "db3" | "ics" | "eml" | "ipynb" => Category::Document,
        "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" | "tbz" | "tbz2" | "txz" | "tzst"
        | "7z" | "rar" => Category::Archive,
        _ => Category::Other,
    }
}

// Evaluated when the visible inventory changes, rather than scanning it on
// every frame. Empty lists have no family header.
pub(super) fn single_family(directory: &Directory, visible: &[usize]) -> Option<Category> {
    let first = family(directory.entries.get(*visible.first()?)?);
    visible
        .iter()
        .all(|&index| family(&directory.entries[index]) == first)
        .then_some(first)
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folders",
            Self::Audio => "Audio",
            Self::VcvRack => "VCV Rack",
            Self::Image => "Images",
            Self::Brushes => "Brushes",
            Self::Video => "Video",
            Self::Scene => "3D & materials",
            Self::Font => "Fonts",
            Self::Geography => "Geography",
            Self::Document => "Documents",
            Self::Text => "Text & code",
            Self::Archive => "Archives",
            Self::Binary => "Binary files",
            Self::Other => "Other files",
            Self::System => "System interfaces",
        }
    }
}

impl Sort {
    pub fn group_at(
        self,
        directory: &Directory,
        visible: &[usize],
        index: usize,
    ) -> Option<Category> {
        if self.column != Column::Type {
            return None;
        }
        let current = family(directory.entries.get(*visible.get(index)?)?);
        let previous = index
            .checked_sub(1)
            .map(|index| family(&directory.entries[visible[index]]));
        (previous != Some(current)).then_some(current)
    }
}

impl FileManager {
    pub(super) fn grouped_by_type(&self) -> bool {
        self.single_family.is_none()
            && self
                .list_sort()
                .is_some_and(|sort| sort.column == Column::Type)
    }
    pub(super) fn list_sort(&self) -> Option<Sort> {
        (self
            .search
            .answers
            .current()
            .is_none_or(|folder| folder.sort.is_none())
            && !self
                .search
                .applied
                .as_ref()
                .is_some_and(|query| for_query(query).is_none()))
        .then_some(self.sort)
    }
    pub(super) fn sort_by(&mut self, column: Column) {
        let active = self.list_sort().is_some()
            && self
                .single_family
                .is_none_or(|family| self.metadata.sort(family).is_none());
        if let Some(family) = self.single_family {
            self.metadata
                .toggle_sort(family, super::metadata::Field::Filename);
        }
        if let Some(folder) = self.search.answers.current_mut() {
            folder.sort = None;
        }
        self.sort = Sort {
            column,
            descending: if active && self.sort.column == column {
                !self.sort.descending
            } else {
                matches!(column, Column::Modified | Column::Created)
            },
        };
        if let Some(query) = &mut self.search.applied {
            query.sort = Some(super::search::query::Sort {
                field: match column {
                    Column::Name => super::search::query::SortField::Name,
                    Column::Type => super::search::query::SortField::Type,
                    Column::Size => super::search::query::SortField::Size,
                    Column::Modified => super::search::query::SortField::Modified,
                    Column::Created => super::search::query::SortField::Created,
                },
                descending: self.sort.descending,
            });
        }
        self.finish_sort();
    }
    pub(super) fn finish_sort(&mut self) {
        self.type_navigation.clear();
        self.gallery_layout.invalidate_order();
        self.rebuild_visible();
        self.reveal_selection = self.selection_count() > 0;
        if let Some(location) = self.location_mut() {
            location.scroll = 0.0;
        }
        self.restore_scroll = true;
    }
}

pub(super) fn for_query(query: &clip_search::query::Query) -> Option<Sort> {
    use clip_search::query::SortField;
    let sort = query.sort?;
    let column = match sort.field {
        SortField::Name => Column::Name,
        SortField::Type => Column::Type,
        SortField::Size => Column::Size,
        SortField::Modified => Column::Modified,
        SortField::Created => Column::Created,
        _ => return None,
    };
    Some(Sort {
        column,
        descending: sort.descending,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_names_order_numeric_runs_without_integer_overflow() {
        let mut names: Vec<_> = (1..=20).rev().map(|n| format!("foo_{n}.jpg")).collect();
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            (1..=20).map(|n| format!("foo_{n}.jpg")).collect::<Vec<_>>()
        );
        assert!(natural_cmp("foo_2.jpg", "foo_02.jpg").is_lt());
        assert!(natural_cmp("v2_part9", "v2_part10").is_lt());
        assert!(
            natural_cmp(
                &format!("file{}", "9".repeat(100)),
                &format!("file1{}", "0".repeat(100))
            )
            .is_lt()
        );
        assert!(natural_cmp("file", "file1").is_lt());
        assert!(natural_cmp("é2", "é10").is_lt());
    }

    fn file_entry(name: &str) -> Entry {
        Entry {
            path: std::path::PathBuf::from(name),
            name: name.to_owned(),
            directory: false,
            regular_file: true,
            system: None,
            typescript: false,
            symlink: false,
            hidden: false,
            size: String::new(),
            size_bytes: 0,
            modified: String::new(),
            modified_time: None,
            created_time: None,
            warning: None,
            revision: 0,
            identity: None,
        }
    }

    #[test]
    fn radiacode_tracks_sort_with_map_documents() {
        for name in ["survey.RCTRK", "route.gpx", "map.geojson"] {
            assert_eq!(family(&file_entry(name)), Category::Document);
        }
    }

    #[test]
    fn vcv_patches_sort_as_dedicated_vcv_rack_category_with_vcv_metadata() {
        let patch = file_entry("ambient.vcv");
        let uppercase_patch = file_entry("AMBIENT.VCV");
        let strip = file_entry("chain.vcvss");
        let uppercase_strip = file_entry("CHAIN.VCVSS");
        let wav = file_entry("loop.wav");

        assert_eq!(family(&patch), Category::VcvRack);
        assert_eq!(family(&uppercase_patch), Category::VcvRack);
        assert_eq!(family(&strip), Category::VcvRack);
        assert_eq!(family(&uppercase_strip), Category::VcvRack);
        assert_eq!(Category::VcvRack.label(), "VCV Rack");
        assert_eq!(family(&wav), Category::Audio);
        assert_eq!(Category::Audio.label(), "Audio");
        assert_eq!(
            super::super::metadata::Field::for_family(Category::VcvRack),
            [
                super::super::metadata::Field::Modules,
                super::super::metadata::Field::Cables,
                super::super::metadata::Field::Rows,
            ]
        );

        let mut categories = [
            Category::Image,
            family(&patch),
            family(&wav),
            Category::Document,
        ];
        categories.sort();
        assert_eq!(
            categories,
            [
                Category::Audio,
                Category::VcvRack,
                Category::Image,
                Category::Document,
            ]
        );
    }
}
