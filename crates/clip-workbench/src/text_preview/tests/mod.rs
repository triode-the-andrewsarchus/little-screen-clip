use super::raw_code::*;
use super::*;

mod benchmark;
mod minimap;
mod raw_code;
mod search;
mod sources;
mod studio;

#[cfg(target_os = "linux")]
pub(super) fn open_spool_files_in(directory: &Path) -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|target| target.starts_with(directory))
        .count()
}

pub(super) fn collect_fragment_ranges(
    spool: &mut SearchMatchSpool,
    start: u64,
    end: u64,
) -> Vec<SearchMatchRange> {
    spool
        .ranges_for_fragment(start, end)
        .unwrap()
        .map(|result| result.map(|(_, range)| range))
        .collect::<io::Result<Vec<_>>>()
        .unwrap()
}

pub(super) fn contrast(a: Color32, b: Color32) -> f32 {
    little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
}
