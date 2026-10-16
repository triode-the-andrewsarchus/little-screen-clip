//! GTK's shared URI/optional-label list. Only worker jobs touch the filesystem.
use super::{BackgroundJobs, FileManager};
use little_clip_core::JobError;
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

const LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Bookmark {
    pub uri: String,
    pub label: String,
    pub path: Option<PathBuf>,
}

#[derive(Clone)]
enum Action {
    Reload,
    Add(Vec<PathBuf>, Option<u64>),
    Remove(String),
}
impl Action {
    fn writing(&self) -> bool {
        !matches!(self, Self::Reload)
    }
    fn token(&self) -> Option<u64> {
        match self {
            Self::Add(_, token) => *token,
            _ => None,
        }
    }
}
type Receiver = mpsc::Receiver<Result<Result<Vec<Bookmark>, String>, JobError>>;

pub(super) struct Bookmarks {
    path: PathBuf,
    legacy: Option<PathBuf>,
    pub entries: Vec<Bookmark>,
    queue: VecDeque<Action>,
    pending: Option<(Action, Receiver)>,
    checked: Option<Instant>,
    last_error: Option<String>,
    pub report: Option<String>,
    pub native_results: Vec<(u64, bool)>,
}
impl Bookmarks {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            legacy: None,
            entries: Vec::new(),
            queue: VecDeque::new(),
            pending: None,
            checked: None,
            last_error: None,
            report: None,
            native_results: Vec::new(),
        }
    }
    pub fn for_user(home: &Path) -> Self {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let mut bookmarks = Self::new(config.join("gtk-3.0/bookmarks"));
        bookmarks.legacy = Some(home.join(".gtk-bookmarks"));
        bookmarks
    }
    pub fn busy(&self) -> bool {
        self.queue.iter().any(Action::writing)
            || self
                .pending
                .as_ref()
                .is_some_and(|(action, _)| action.writing())
    }
    pub fn add(&mut self, paths: Vec<PathBuf>, token: Option<u64>) -> bool {
        if paths.is_empty() || self.queue.len() >= 64 {
            return false;
        }
        self.queue.push_back(Action::Add(paths, token));
        true
    }
    pub fn remove(&mut self, uri: String) {
        if self.queue.len() < 64 {
            self.queue.push_back(Action::Remove(uri));
        }
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        let result = self
            .pending
            .as_ref()
            .and_then(|(_, receiver)| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(JobError::Cancelled)),
            });
        if let Some(result) = result
            && let Some((action, _)) = self.pending.take()
        {
            self.complete(action, result);
        }
        if self.pending.is_none() {
            let action = self.queue.pop_front().or_else(|| {
                self.checked
                    .is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
                    .then_some(Action::Reload)
            });
            if let Some(action) = action {
                self.checked = Some(Instant::now());
                let (path, legacy, work) = (self.path.clone(), self.legacy.clone(), action.clone());
                // Accepted writes must keep their completion even on slow storage.
                let deadline = if action.writing() {
                    Duration::MAX
                } else {
                    Duration::from_secs(5)
                };
                let (_, receiver) =
                    jobs.submit(deadline, move || run(&path, legacy.as_deref(), &work));
                self.pending = Some((action, receiver));
            }
        }
    }
    fn complete(
        &mut self,
        action: Action,
        result: Result<Result<Vec<Bookmark>, String>, JobError>,
    ) {
        if matches!(result, Err(JobError::QueueFull)) {
            self.queue.push_front(action);
            return;
        }
        let result = result
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        if let Some(token) = action.token() {
            self.native_results.push((token, result.is_ok()));
        }
        match result {
            Ok(entries) => {
                self.entries = entries;
                self.last_error = None;
            }
            Err(error) => {
                if action.writing() || self.last_error.as_ref() != Some(&error) {
                    self.report = Some(format!("Bookmarks: {error}"));
                }
                self.last_error = Some(error);
            }
        }
    }
}

fn read(path: &Path) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    crate::file_source::open_regular_file(path, true)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(std::io::Error::other("Bookmark list exceeds 1 MiB"));
    }
    String::from_utf8(bytes).map_err(std::io::Error::other)
}
fn run(path: &Path, legacy: Option<&Path>, action: &Action) -> Result<Vec<Bookmark>, String> {
    let text = match read(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match legacy.map(read) {
            Some(Ok(text)) => text,
            Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error.to_string());
            }
            _ => String::new(),
        },
        result => result.map_err(|error| error.to_string())?,
    };
    let updated = match action {
        Action::Reload => return Ok(parse(&text)),
        Action::Add(paths, _) => add_paths(&text, paths)?,
        Action::Remove(uri) => text
            .split_inclusive('\n')
            .filter(|line| uri_token(line) != uri)
            .collect(),
    };
    if updated != text {
        write(path, &updated).map_err(|error| error.to_string())?;
    }
    Ok(parse(&updated))
}
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if text.len() as u64 > LIMIT {
        return Err(std::io::Error::other("Bookmark list exceeds 1 MiB"));
    }
    // Keep a dotfile-manager symlink intact when replacing an existing list.
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error),
    };
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("No bookmark directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        staged.as_file().set_permissions(metadata.permissions())?;
    }
    staged.write_all(text.as_bytes())?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(())
}
fn uri_token(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
        .split_once(' ')
        .map_or(line.trim_end_matches(['\r', '\n']), |(uri, _)| uri)
}
fn parse(text: &str) -> Vec<Bookmark> {
    text.lines()
        .filter_map(|line| {
            let (uri, label) = line
                .split_once(' ')
                .map_or((line, None), |(uri, label)| (uri, Some(label)));
            let url = url::Url::parse(uri).ok()?;
            let path = url.to_file_path().ok();
            let label = label
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    path.as_ref().map_or_else(
                        || uri.to_owned(),
                        |path| super::display_path(path.file_name().map(Path::new).unwrap_or(path)),
                    )
                });
            Some(Bookmark {
                uri: uri.into(),
                label,
                path,
            })
        })
        .collect()
}
fn add_paths(text: &str, paths: &[PathBuf]) -> Result<String, String> {
    let mut result = text.to_owned();
    let mut known: std::collections::BTreeSet<_> =
        parse(text).into_iter().map(|entry| entry.uri).collect();
    for path in paths {
        if !std::fs::metadata(path)
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err("Only folders can be bookmarked".into());
        }
        let uri = url::Url::from_file_path(path)
            .map_err(|_| "Bookmark needs an absolute folder path")?
            .to_string();
        if known.insert(uri.clone()) {
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&uri);
            result.push('\n');
        }
    }
    Ok(result)
}

impl FileManager {
    pub(super) fn selected_bookmark_folders(&self) -> Option<Vec<PathBuf>> {
        let selection = &self.location()?.selection;
        if selection.is_empty() {
            return None;
        }
        let directory = self.directory.as_ref()?;
        let folders: Vec<_> = directory
            .entries
            .iter()
            .filter(|entry| entry.directory && selection.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        (folders.len() == selection.len()).then_some(folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gtk_labels_encoded_paths_and_remote_references_in_order() {
        let entries = parse(
            "file:///tmp/Some%20Folder My folder\nfile:///tmp/plain\nsftp://host/share Remote\n# comment\nnot a URI\n",
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].label, "My folder");
        assert_eq!(
            entries[0].path.as_deref(),
            Some(Path::new("/tmp/Some Folder"))
        );
        assert_eq!(entries[1].label, "plain");
        assert_eq!(entries[2].label, "Remote");
        assert!(entries[2].path.is_none());
    }

    #[test]
    fn adds_and_removes_only_references_preserving_other_app_changes_and_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("Some Folder");
        std::fs::create_dir(&folder).unwrap();
        // Adding a shortcut must never inventory a folder's contents.
        std::os::unix::fs::symlink("/sys", folder.join("kernel-link")).unwrap();
        let list = directory.path().join("gtk-3.0/bookmarks");
        let entries = run(&list, None, &Action::Add(vec![folder.clone()], None)).unwrap();
        let uri = entries[0].uri.clone();
        assert!(uri.contains("Some%20Folder"));
        let original = std::fs::read_to_string(&list).unwrap();
        run(&list, None, &Action::Add(vec![folder.clone()], None)).unwrap();
        assert_eq!(std::fs::read_to_string(&list).unwrap(), original);
        // Simulate another file manager adding a labeled remote bookmark.
        let external = "sftp://host/share Shared server\n# keep this comment\n";
        std::fs::write(&list, format!("{original}{external}")).unwrap();
        let remaining = run(&list, None, &Action::Remove(uri)).unwrap();
        assert_eq!(std::fs::read_to_string(&list).unwrap(), external);
        assert_eq!(remaining[0].label, "Shared server");
        assert!(folder.is_dir());
        assert!(folder.join("kernel-link").is_symlink());
    }

    #[test]
    fn rejects_mixed_batches_without_writing_and_preserves_bookmark_file_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let file = directory.path().join("file");
        std::fs::write(&file, "content").unwrap();
        let target = directory.path().join("real-bookmarks");
        let list = directory.path().join("bookmarks");
        std::fs::write(&target, "sftp://host/ Remote\n").unwrap();
        std::os::unix::fs::symlink(&target, &list).unwrap();
        assert!(run(&list, None, &Action::Add(vec![folder.clone(), file], None)).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "sftp://host/ Remote\n"
        );
        run(&list, None, &Action::Add(vec![folder], None)).unwrap();
        assert!(list.is_symlink());
        assert_eq!(parse(&std::fs::read_to_string(&target).unwrap()).len(), 2);
    }

    #[test]
    fn asynchronous_reload_observes_external_atomic_replacements_and_removal() {
        let directory = tempfile::tempdir().unwrap();
        let list = directory.path().join("bookmarks");
        let mut bookmarks = Bookmarks::new(list.clone());
        let jobs = BackgroundJobs::new(1, 8);
        let finish = |bookmarks: &mut Bookmarks| {
            bookmarks.checked = None;
            bookmarks.tick(&jobs);
            let deadline = Instant::now() + Duration::from_secs(5);
            while bookmarks.pending.is_some() {
                bookmarks.tick(&jobs);
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        finish(&mut bookmarks);
        assert!(bookmarks.entries.is_empty());
        write(&list, "file:///tmp First label\n").unwrap();
        finish(&mut bookmarks);
        assert_eq!(bookmarks.entries[0].label, "First label");
        write(&list, "file:///tmp Second label\n").unwrap();
        finish(&mut bookmarks);
        assert_eq!(bookmarks.entries[0].label, "Second label");
        std::fs::remove_file(list).unwrap();
        finish(&mut bookmarks);
        assert!(bookmarks.entries.is_empty());
    }
}
