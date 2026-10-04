use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{BackgroundJobs, Completion, Directory, DirectoryJob};

#[derive(Default)]
pub(super) struct Branch {
    pub expanded: bool,
    pub directory: Option<Directory>,
    pub error: Option<String>,
    job: Option<DirectoryJob>,
}

pub(super) struct TreeRow {
    pub path: PathBuf,
    pub depth: usize,
    pub home: bool,
}

pub(super) struct FolderTree {
    home: PathBuf,
    pub branches: BTreeMap<PathBuf, Branch>,
    pub selected: Option<PathBuf>,
    pub reveal: bool,
    ancestors: BTreeSet<PathBuf>,
}

impl FolderTree {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            branches: BTreeMap::new(),
            selected: None,
            reveal: false,
            ancestors: BTreeSet::new(),
        }
    }

    pub fn reveal(&mut self, path: &Path) {
        self.ancestors = path.ancestors().map(Path::to_path_buf).collect();
        for parent in path.ancestors().skip(1) {
            self.branches.entry(parent.into()).or_default().expanded = true;
        }
        self.selected = Some(path.into());
        self.reveal = true;
    }

    pub fn toggle(&mut self, path: PathBuf) {
        let branch = self.branches.entry(path.clone()).or_default();
        branch.expanded = !branch.expanded;
        if branch.expanded {
            branch.error = None;
        }
        if !branch.expanded {
            branch.job = None;
            // Collapsed descendants release both cached listings and pending jobs.
            self.branches
                .retain(|child, _| child == &path || !child.starts_with(&path));
        }
    }

    pub fn refresh(&mut self, path: PathBuf) {
        self.branches
            .retain(|child, _| child == &path || !child.starts_with(&path));
        if let Some(branch) = self.branches.get_mut(&path) {
            branch.job = None;
            branch.directory = None;
            branch.error = None;
        }
    }

    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        let mut running = self
            .branches
            .values()
            .filter(|branch| branch.job.is_some())
            .count();
        for (path, branch) in &mut self.branches {
            if let Some(job) = &branch.job
                && let Some(result) = job.poll()
            {
                branch.job = None;
                running -= 1;
                match result {
                    Completion::Retry => {}
                    Completion::Ready(Ok(directory)) => {
                        self.reveal |= self.ancestors.contains(path);
                        // Symlink aliases may point back to an ancestor. Show the
                        // alias as an activatable row, never recursively expand it.
                        if path != &directory.path && path.starts_with(&directory.path) {
                            branch.error =
                                Some("Link points to an ancestor; open it to navigate".into());
                        } else {
                            branch.directory = Some(directory);
                        }
                    }
                    Completion::Ready(Err(error)) => branch.error = Some(error),
                }
            }
            if running < 2
                && branch.expanded
                && branch.directory.is_none()
                && branch.error.is_none()
                && branch.job.is_none()
            {
                branch.job = Some(DirectoryJob::new(path.clone(), jobs));
                running += 1;
            }
        }
    }

    pub fn rows(&self, show_hidden: bool) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        let mut stack = vec![(PathBuf::from("/"), 0, false)];
        if self.home != Path::new("/") {
            stack.push((self.home.clone(), 0, true));
        }
        let mut visited = BTreeSet::new();
        while let Some((path, depth, home)) = stack.pop() {
            // A duplicate shortcut is visible but its subtree appears only once.
            let first = visited.insert(path.clone());
            if first
                && let Some(branch) = self.branches.get(&path)
                && branch.expanded
            {
                for child in self.children(&path, show_hidden).into_iter().rev() {
                    stack.push((child.to_path_buf(), depth + 1, false));
                }
            }
            rows.push(TreeRow { path, depth, home });
        }
        rows
    }

    // An explicitly opened path remains reachable in the tree even while its
    // ancestors load, or when a parent permits traversal but denies enumeration.
    fn children(&self, path: &Path, show_hidden: bool) -> Vec<&Path> {
        let mut children: Vec<_> = self
            .branches
            .get(path)
            .and_then(|branch| branch.directory.as_ref())
            .into_iter()
            .flat_map(|directory| &directory.entries)
            .filter(|entry| {
                entry.directory
                    && (show_hidden || !entry.hidden || self.ancestors.contains(&entry.path))
            })
            .map(|entry| entry.path.as_path())
            .collect();
        if let Some(known) = self
            .ancestors
            .iter()
            .find(|ancestor| ancestor.parent() == Some(path))
            && !children.contains(&known.as_path())
        {
            children.push(known);
        }
        children.sort_unstable();
        children
    }
}
