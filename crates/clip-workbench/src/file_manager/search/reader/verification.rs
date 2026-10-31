//! Final source validation shared by indexed and on-demand content results.
use super::{Entry, Reply, Request, SourceRoot};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

pub(in crate::file_manager::search) fn current(
    entries: &[Entry],
    roots: &[SourceRoot],
    cancel: &AtomicBool,
) -> Result<HashSet<PathBuf>, String> {
    with_reader(entries, roots, cancel, super::call)
}

fn with_reader(
    entries: &[Entry],
    roots: &[SourceRoot],
    cancel: &AtomicBool,
    mut read: impl FnMut(&Request, &AtomicBool) -> Result<Reply, String>,
) -> Result<HashSet<PathBuf>, String> {
    let mut verified = HashSet::new();
    for root in roots {
        // Literal searches may contain more than the semantic candidate cap.
        // Keep each request within the existing reader's 1,000-entry budget.
        for chunk in entries.chunks(1000) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Search cancelled".into());
            }
            let entries = chunk
                .iter()
                .filter(|entry| entry.path.starts_with(&root.path))
                .cloned()
                .collect::<Vec<_>>();
            if entries.is_empty() {
                continue;
            }
            let request = Request::VerifyBatch {
                root: root.clone(),
                entries,
            };
            if let Ok(Reply::VerifiedBatch(indices)) = read(&request, cancel) {
                let Request::VerifyBatch { entries, .. } = &request else {
                    unreachable!();
                };
                for index in indices {
                    if let Some(entry) = entries.get(index) {
                        verified.insert(entry.path.clone());
                    }
                }
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Search cancelled".into());
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::{filesystem, search::policy::Root};

    #[test]
    fn final_batches_reject_changed_sources_without_hiding_unchanged_or_later_results() {
        let directory = tempfile::tempdir().unwrap();
        for number in 0..1002 {
            std::fs::write(directory.path().join(format!("{number}.txt")), "original").unwrap();
        }
        let root = Root::open(directory.path()).unwrap();
        let source = SourceRoot::from_root(&root);
        let entries = filesystem::read_directory(directory.path(), &AtomicBool::new(false))
            .unwrap()
            .entries;
        std::fs::write(&entries[0].path, "changed content").unwrap();
        std::fs::remove_file(&entries[1001].path).unwrap();
        let mut batches = Vec::new();
        let verified = with_reader(
            &entries,
            &[source],
            &AtomicBool::new(false),
            |request, _| {
                let Request::VerifyBatch { entries, .. } = request else {
                    panic!("wrong reader operation");
                };
                batches.push(entries.len());
                // Run the actual pinned-source/root checks, without spawning the
                // unit-test harness as though it were the application helper.
                super::super::execute(request.clone())
            },
        )
        .unwrap();
        assert_eq!(batches, [1000, 2]);
        assert_eq!(verified.len(), 1000);
        assert!(!verified.contains(&entries[0].path));
        assert!(!verified.contains(&entries[1001].path));
        assert!(verified.contains(&entries[1000].path));
        assert!(
            entries[1..1001]
                .iter()
                .all(|entry| verified.contains(&entry.path))
        );
    }

    #[test]
    fn failed_root_does_not_block_another_root_and_cancelled_checks_never_publish() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("current.txt"), "current").unwrap();
        let (source, entry) = super::super::tests::only_source(directory.path());
        let entries = vec![entry];
        let mut unavailable = source.clone();
        unavailable.mount = None;
        let stop = AtomicBool::new(false);
        let verified = with_reader(
            &entries,
            &[unavailable, source.clone()],
            &stop,
            |request, _| super::super::execute(request.clone()),
        )
        .unwrap();
        assert_eq!(verified.len(), 1);
        let result = with_reader(
            &entries,
            std::slice::from_ref(&source),
            &stop,
            |_, cancel| {
                cancel.store(true, Ordering::Relaxed);
                Ok(Reply::VerifiedBatch(vec![0]))
            },
        );
        assert!(matches!(result, Err(error) if error.contains("cancelled")));
        assert!(
            with_reader(&entries, &[source], &stop, |_, _| panic!(
                "cancelled check dispatched"
            ))
            .is_err()
        );
    }
}
