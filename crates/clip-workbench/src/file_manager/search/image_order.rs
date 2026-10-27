//! Session image ordering, independent from semantic enrollment and inference.
use super::{
    query::{Query, SortField},
    reader::{self, Reply, Request},
};
use crate::file_manager::{
    filesystem::{Directory, Entry},
    preview_job::Task,
    previews::EntryPreviews,
    sorting,
};
use little_clip_core::{BackgroundJobs, JobError};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    time::{Duration, Instant},
};

type Key = (PathBuf, u64);
#[derive(Default)]
pub(in crate::file_manager) struct ImageOrder {
    active: bool,
    suspended: bool,
    reference: Option<Entry>,
    reference_checked: bool,
    reference_check: Option<Instant>,
    invalid: bool,
    values: HashMap<Key, Result<u64, String>>,
    pending: Vec<(Entry, Task<Result<u64, String>>)>,
    priority: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn encoded_color_and_version_fields_are_not_part_of_dhash() {
        let parse = crate::file_manager::image_strip::parse_dhash;
        let bits = 0x0123_4567_89ab_cdef;
        assert_eq!(parse(&format!("1000{bits:016x}")), Some(bits));
        assert_eq!(parse(&format!("1fff{bits:016x}")), Some(bits));
        assert_eq!(parse(&format!("2000{bits:016x}")), None);
        assert_eq!(parse("1fff000000000000000g"), None);
        assert_eq!(parse("0000000000000000"), None);
    }

    fn fixture() -> (tempfile::TempDir, Directory) {
        let folder = tempfile::tempdir().unwrap();
        for name in [
            "reference.png",
            "near.png",
            "foo_2.png",
            "foo_10.png",
            "bad.png",
            "pending.png",
        ] {
            std::fs::write(folder.path().join(name), "fixture").unwrap();
        }
        let directory =
            crate::file_manager::filesystem::read_directory(folder.path(), &AtomicBool::new(false))
                .unwrap();
        (folder, directory)
    }
    #[test]
    fn distance_orders_xor_bits_and_keeps_missing_last_in_both_directions() {
        let (_folder, directory) = fixture();
        let reference = directory
            .entries
            .iter()
            .find(|entry| entry.name == "reference.png")
            .unwrap();
        let query = Query::parse(":image;sort:similarity;ref:selected").unwrap();
        let mut order = ImageOrder::default();
        order
            .begin(
                &query,
                &directory,
                &BTreeSet::from([reference.path.clone()]),
            )
            .unwrap();
        order.reference_checked = true;
        for entry in &directory.entries {
            let value = match entry.name.as_str() {
                "reference.png" => Ok(u64::MAX),
                "near.png" => Ok(u64::MAX ^ 1),
                "foo_2.png" | "foo_10.png" => Ok(0),
                "bad.png" => Err("decoder unavailable".into()),
                _ => continue,
            };
            order.values.insert(ImageOrder::key(entry), value);
        }
        let mut visible = (0..directory.entries.len()).collect::<Vec<_>>();
        order.order(&directory, &mut visible, false);
        let names = |indices: &[usize]| {
            indices
                .iter()
                .map(|&index| directory.entries[index].name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&visible),
            [
                "reference.png",
                "near.png",
                "foo_2.png",
                "foo_10.png",
                "bad.png",
                "pending.png"
            ]
        );
        assert_eq!(order.cell(reference), "0 / 64");
        order.order(&directory, &mut visible, true);
        assert_eq!(
            names(&visible),
            [
                "foo_2.png",
                "foo_10.png",
                "near.png",
                "reference.png",
                "bad.png",
                "pending.png"
            ]
        );
    }
    #[test]
    fn raw_hash_order_is_unsigned_and_an_invalid_reference_keeps_the_prior_request() {
        let (_folder, directory) = fixture();
        let mut order = ImageOrder::default();
        order
            .begin(
                &Query::parse(":image;sort:dhash").unwrap(),
                &directory,
                &BTreeSet::new(),
            )
            .unwrap();
        let a = &directory.entries[0];
        let b = &directory.entries[1];
        order.values.insert(ImageOrder::key(a), Ok(u64::MAX));
        order.values.insert(ImageOrder::key(b), Ok(1_u64 << 63));
        let mut visible = vec![0, 1];
        order.order(&directory, &mut visible, false);
        assert_eq!(visible, [1, 0]);
        let query = Query::parse(":image;sort:similarity;ref:selected").unwrap();
        assert!(order.begin(&query, &directory, &BTreeSet::new()).is_err());
        assert!(order.active());
        assert_eq!(order.heading(), "dHash");
        assert_eq!(order.value(a), Some(u64::MAX));
    }
    #[test]
    fn refreshing_a_replaced_reference_invalidates_instead_of_switching_reference() {
        let (_folder, mut directory) = fixture();
        let reference = directory.entries[0].clone();
        let mut order = ImageOrder::default();
        order
            .begin(
                &Query::parse(":image;sort:similarity;ref:selected").unwrap(),
                &directory,
                &BTreeSet::from([reference.path.clone()]),
            )
            .unwrap();
        directory.entries[0].revision = reference.revision.wrapping_add(1);
        let visible = (0..directory.entries.len()).collect::<Vec<_>>();
        let pool = BackgroundJobs::new(1, 2);
        let (changed, error) = order.tick(
            &directory,
            &directory,
            &visible,
            &EntryPreviews::default(),
            &pool,
        );
        assert!(changed);
        assert!(error.unwrap().contains("Reference changed"));
        assert!(order.invalid);
        assert!(order.pending.is_empty());
        assert_eq!(
            order.reference.as_ref().unwrap().revision,
            reference.revision
        );
    }
}
impl ImageOrder {
    pub fn begin(
        &mut self,
        query: &Query,
        directory: &Directory,
        selected: &BTreeSet<PathBuf>,
    ) -> Result<(), String> {
        let reference = if query
            .sort
            .is_some_and(|sort| sort.field == SortField::Similarity)
        {
            if selected.len() != 1 {
                return Err("Similarity sorting needs exactly one selected image".into());
            }
            let entry = directory
                .entries
                .iter()
                .find(|entry| selected.contains(&entry.path))
                .filter(|entry| entry.is_photo() && !entry.symlink && entry.identity.is_some())
                .ok_or("Select an ordinary image as the similarity reference")?;
            Some(entry.clone())
        } else {
            None
        };
        self.retire();
        self.values.retain(|_, value| value.is_ok());
        self.active = query
            .sort
            .is_some_and(|sort| matches!(sort.field, SortField::Dhash | SortField::Similarity));
        self.reference = reference;
        Ok(())
    }
    pub fn retire(&mut self) {
        self.active = false;
        self.pending.clear();
        self.reference = None;
        self.reference_checked = false;
        self.reference_check = None;
        self.invalid = false;
        self.priority.clear();
        if self.values.len() > 8192 {
            self.values.clear();
        }
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn set_suspended(&mut self, suspended: bool) {
        if suspended {
            self.pending.clear();
        }
        self.suspended = suspended;
    }
    pub fn prioritize(&mut self, paths: Vec<PathBuf>) {
        self.priority = paths;
    }
    pub fn heading(&self) -> &'static str {
        if self.reference.is_some() {
            "Distance"
        } else {
            "dHash"
        }
    }
    fn key(entry: &Entry) -> Key {
        (entry.path.clone(), entry.revision)
    }
    fn hash(&self, entry: &Entry) -> Option<u64> {
        self.values.get(&Self::key(entry))?.as_ref().ok().copied()
    }
    pub fn value(&self, entry: &Entry) -> Option<u64> {
        if self.invalid {
            return None;
        }
        let hash = self.hash(entry)?;
        match &self.reference {
            Some(reference) if self.reference_checked => {
                Some(u64::from((hash ^ self.hash(reference)?).count_ones()))
            }
            Some(_) => None,
            None => Some(hash),
        }
    }
    pub fn cell(&self, entry: &Entry) -> String {
        if self.invalid {
            return "Unavailable".into();
        }
        match self.value(entry) {
            Some(value) if self.reference.is_some() => format!("{value} / 64"),
            Some(value) => format!("{value:016x}"),
            None if !entry.is_photo() || entry.symlink => "—".into(),
            None if self
                .values
                .get(&Self::key(entry))
                .is_some_and(Result::is_err) =>
            {
                "Failed".into()
            }
            None => "Pending".into(),
        }
    }
    pub fn order(&self, directory: &Directory, visible: &mut Vec<usize>, descending: bool) {
        sorting::Sort::default().apply(directory, visible);
        visible.sort_by(|&a, &b| {
            sorting::available(
                self.value(&directory.entries[a]),
                self.value(&directory.entries[b]),
                descending,
            )
        });
    }
    pub fn status(&self, directory: &Directory, visible: &[usize]) -> String {
        let images = visible
            .iter()
            .map(|&index| &directory.entries[index])
            .filter(|entry| entry.is_photo() && !entry.symlink)
            .collect::<Vec<_>>();
        let done = images
            .iter()
            .filter(|entry| self.values.contains_key(&Self::key(entry)))
            .count();
        let failures = images
            .iter()
            .filter(|entry| {
                self.values
                    .get(&Self::key(entry))
                    .is_some_and(Result::is_err)
            })
            .count();
        match &self.reference {
            Some(reference) => format!(
                "Reference: {} · {done}/{} images analyzed · {failures} failed · distance 0–64",
                reference.name,
                images.len()
            ),
            None => format!(
                "{done}/{} images analyzed · {failures} failed · unsigned dHash-64 order",
                images.len()
            ),
        }
    }
    pub fn tick(
        &mut self,
        directory: &Directory,
        origin: &Directory,
        visible: &[usize],
        previews: &EntryPreviews,
        jobs: &BackgroundJobs,
    ) -> (bool, Option<String>) {
        if !self.active || self.invalid || self.suspended {
            return (false, None);
        }
        if let Some(reference) = &self.reference
            && !origin.entries.iter().any(|entry| {
                entry.path == reference.path
                    && entry.revision == reference.revision
                    && entry.identity == reference.identity
            })
        {
            return self.invalidate(
                "Reference changed or disappeared; select an image and reapply the query",
            );
        }
        let changed = self.poll();
        if self.reference.as_ref().is_some_and(|reference| {
            self.values
                .get(&Self::key(reference))
                .is_some_and(Result::is_err)
        }) {
            return self.invalidate(
                "Reference could not be verified; select an image and reapply the query",
            );
        }
        let mut candidates = visible
            .iter()
            .map(|&index| &directory.entries[index])
            .filter(|entry| entry.is_photo() && !entry.symlink)
            .collect::<Vec<_>>();
        candidates.sort_by_key(|entry| {
            self.priority
                .iter()
                .position(|path| path == &entry.path)
                .unwrap_or(usize::MAX)
        });
        let reference = self.reference.clone();
        if let Some(reference) = &reference {
            candidates.insert(0, reference);
        }
        let cached = self.admit(candidates, previews, jobs);
        (changed || cached, None)
    }
    fn invalidate(&mut self, message: &str) -> (bool, Option<String>) {
        self.invalid = true;
        self.pending.clear();
        (true, Some(message.into()))
    }
    fn poll(&mut self) -> bool {
        let mut changed = false;
        for index in (0..self.pending.len()).rev() {
            let Some(result) = self.pending[index].1.poll() else {
                continue;
            };
            let (entry, _) = self.pending.swap_remove(index);
            if matches!(result, Err(JobError::QueueFull)) {
                continue;
            }
            let value = result
                .map_err(|error| error.to_string())
                .and_then(|value| value);
            changed |= self.values.get(&Self::key(&entry)) != Some(&value);
            if self
                .reference
                .as_ref()
                .is_some_and(|reference| Self::key(reference) == Self::key(&entry))
            {
                changed |= !self.reference_checked && value.is_ok();
                self.reference_checked = value.is_ok();
                self.reference_check = Some(Instant::now());
            }
            self.values.insert(Self::key(&entry), value);
        }
        changed
    }
    fn admit(
        &mut self,
        candidates: Vec<&Entry>,
        previews: &EntryPreviews,
        jobs: &BackgroundJobs,
    ) -> bool {
        let previous = self.values.len();
        for entry in candidates {
            let key = Self::key(entry);
            let reference = self
                .reference
                .as_ref()
                .is_some_and(|reference| Self::key(reference) == key);
            if self
                .pending
                .iter()
                .any(|(pending, _)| Self::key(pending) == key)
            {
                continue;
            }
            let fresh_reference = self.reference_checked
                && self
                    .reference_check
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(1));
            if self.values.contains_key(&key) && (!reference || fresh_reference) {
                continue;
            }
            let cached = self.hash(entry).or_else(|| previews.cached_dhash(entry));
            if !reference && let Some(hash) = cached {
                self.values.insert(key, Ok(hash));
                continue;
            }
            if self.pending.len() >= 2 {
                break;
            }
            let source = entry.clone();
            let task = Task::start(
                jobs,
                Duration::from_secs(95),
                move |cancel| match reader::call(
                    &Request::ImageHash {
                        entry: source,
                        cached,
                    },
                    cancel,
                )? {
                    Reply::ImageHash(hash) => Ok(hash),
                    _ => Err("Invalid image hash reply".into()),
                },
            );
            self.pending.push((entry.clone(), task));
        }
        self.values.len() != previous
    }
}
