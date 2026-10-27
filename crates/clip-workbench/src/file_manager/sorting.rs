//! In-memory browser ordering. No file probing or preview readiness dependencies.
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
                let kind = (self.column == Column::Type).then(|| content_type(entry));
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

/// Compare digit runs by value without parsing integers or imposing a digit limit.
/// Equal numbers with fewer leading zeroes come first. Other text is literal.
pub(super) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    while let (Some(&left), Some(&right)) = (a.first(), b.first()) {
        if left.is_ascii_digit() && right.is_ascii_digit() {
            let (ad, ar) = digits(a);
            let (bd, br) = digits(b);
            let av = &ad[ad.iter().position(|v| *v != b'0').unwrap_or(ad.len())..];
            let bv = &bd[bd.iter().position(|v| *v != b'0').unwrap_or(bd.len())..];
            let order = av
                .len()
                .cmp(&bv.len())
                .then_with(|| av.cmp(bv))
                .then(ad.len().cmp(&bd.len()));
            if !order.is_eq() {
                return order;
            }
            (a, b) = (ar, br);
        } else {
            let order = left.cmp(&right);
            if !order.is_eq() {
                return order;
            }
            a = &a[1..];
            b = &b[1..];
        }
    }
    a.len().cmp(&b.len())
}

fn digits(bytes: &[u8]) -> (&[u8], &[u8]) {
    bytes.split_at(
        bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count(),
    )
}

// Categories deliberately group related extensions; extensions stay contiguous
// inside each category, with natural names used only as the final tie-breaker.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Category {
    Folder,
    Audio,
    Image,
    Video,
    Scene,
    Font,
    Document,
    Text,
    Archive,
    Binary,
    Other,
    System,
}

fn content_type(entry: &Entry) -> (Category, String) {
    let extension = entry
        .path
        .extension()
        .map(|v| v.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let category = if entry.directory {
        Category::Folder
    } else if entry.system.is_some() || !entry.regular_file {
        Category::System
    } else if little_clip_core::audio::is_audio_path(&entry.path)
        || super::midi_strip::is_midi(&entry.path)
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
    } else if super::text_strip::known_text(&entry.path) {
        Category::Text
    } else if super::binary_strip::known(&entry.path) {
        Category::Binary
    } else {
        extension_category(&extension)
    };
    // Folder suffixes do not make them different content types.
    (
        category,
        if entry.directory {
            String::new()
        } else {
            extension
        },
    )
}

fn extension_category(extension: &str) -> Category {
    match extension {
        "pdf" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
        | "epub" => Category::Document,
        "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" => Category::Archive,
        _ => Category::Other,
    }
}

impl FileManager {
    pub(super) fn sort_by(&mut self, column: Column) {
        let answer_active = self
            .search
            .answers
            .current()
            .is_some_and(|folder| folder.sort.is_some());
        if let Some(folder) = self.search.answers.current_mut() {
            folder.sort = None;
        }
        let active = !answer_active
            && !self
                .search
                .applied
                .as_ref()
                .is_some_and(|query| query.browser_sort().is_none());
        self.sort = Sort {
            column,
            descending: if active && self.sort.column == column {
                !self.sort.descending
            } else {
                matches!(column, Column::Modified | Column::Created)
            },
        };
        self.type_navigation.clear();
        self.gallery_layout.invalidate_order();
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
        self.rebuild_visible();
        self.reveal_selection = self.selection_count() > 0;
        if let Some(location) = self.location_mut() {
            location.scroll = 0.0;
        }
        self.restore_scroll = true;
    }
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
}
