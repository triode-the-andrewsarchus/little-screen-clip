//! Home visits are virtual history entries with a retained real folder context.
use super::{FileManager, LocationKind, Navigation};
impl FileManager {
    pub(super) fn show_home(&mut self) {
        if self
            .location()
            .is_some_and(|location| location.kind == LocationKind::Home)
        {
            self.activate_home(self.index.expect("Home history index"));
            return;
        }
        if let Some(location) = self.location().cloned() {
            self.history.truncate(self.index.map_or(0, |i| i + 1));
            self.history.push(location);
            self.index = Some(self.history.len() - 1);
        } else {
            // This is a real folder context, never the virtual Home's pathname.
            self.append_location(self.home.clone());
        }
        let index = self.index.expect("new Home history index");
        self.history[index].kind = LocationKind::Home;
        self.activate_home(index);
    }
    pub(super) fn activate_home(&mut self, index: usize) {
        self.index = Some(index);
        self.archives.cancel();
        self.pending = None;
        self.watch = None;
        self.containers.retire();
        self.stop_previews();
        if self.search.open {
            self.close_search();
        }
        self.home_surface.open = true;
        self.home_surface.reload();
        self.error = None;
        self.failed = None;
    }
    pub(super) fn return_from_home(&mut self) {
        if !self.home_surface.session.allow_leave() {
            return;
        }
        self.home_surface.session.commit_labels();
        self.home_surface.session.canvas.suspend();
        let previous = self.index.and_then(|index| {
            self.history[..index]
                .iter()
                .rposition(|location| location.kind == LocationKind::Folder)
        });
        if let Some(index) = previous {
            self.navigate(self.history[index].path.clone(), Navigation::History(index));
        } else {
            self.navigate(self.path().to_path_buf(), Navigation::New);
        }
        self.focus_list = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::BackgroundJobs;
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    fn finish(browser: &mut FileManager, jobs: &BackgroundJobs) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            browser.tick(jobs);
            if browser.pending.is_none() {
                return;
            }
            assert!(Instant::now() < deadline, "navigation did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn loaded(path: &Path) -> (FileManager, BackgroundJobs) {
        let jobs = BackgroundJobs::new(1, 4);
        let mut browser = FileManager::new(path.into());
        finish(&mut browser, &jobs);
        (browser, jobs)
    }
    #[test]
    fn virtual_visits_preserve_folder_selection_scroll_and_forward_history() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.txt");
        std::fs::write(&path, "note").unwrap();
        let (mut browser, jobs) = loaded(root.path());
        browser.select(path.clone());
        browser.location_mut().unwrap().scroll = 180.0;
        browser.show_home();
        assert_eq!(browser.history.len(), 2);
        assert_eq!(browser.location().unwrap().kind, LocationKind::Home);
        assert_eq!(browser.path(), root.path());
        assert!(browser.pending.is_none());
        browser.history_step(false);
        finish(&mut browser, &jobs);
        assert!(!browser.home_surface.open);
        assert!(browser.location().unwrap().selection.contains(&path));
        assert_eq!(browser.location().unwrap().scroll, 180.0);
        browser.history_step(true);
        assert!(browser.home_surface.open);
        assert_eq!(browser.index, Some(1));
        assert!(
            browser.pending.is_none(),
            "returning to Home does not read a fake directory"
        );
        browser.return_from_home();
        finish(&mut browser, &jobs);
        assert_eq!(browser.index, Some(0));
        assert_eq!(
            browser.history.len(),
            2,
            "Back to folder preserves the forward Home visit"
        );
    }
    #[test]
    fn startup_home_has_no_fabricated_path_and_can_open_its_real_folder_context() {
        let root = tempfile::tempdir().unwrap();
        let jobs = BackgroundJobs::new(1, 4);
        let mut browser = FileManager::new(root.path().into());
        browser.show_home();
        browser.show_home();
        assert_eq!(browser.history.len(), 1);
        assert!(browser.directory.is_none());
        assert!(browser.home_surface.open);
        assert_eq!(browser.path(), root.path());
        browser.return_from_home();
        finish(&mut browser, &jobs);
        assert_eq!(browser.index, Some(1));
        assert_eq!(browser.location().unwrap().kind, LocationKind::Folder);
        browser.history_step(false);
        assert!(browser.home_surface.open);
        assert_eq!(browser.index, Some(0));
    }
    #[test]
    fn ordinary_navigation_from_home_is_a_new_visit_even_for_the_same_folder() {
        let root = tempfile::tempdir().unwrap();
        let (mut browser, jobs) = loaded(root.path());
        browser.show_home();
        browser.navigate(root.path().into(), Navigation::New);
        assert!(browser.pending.is_some());
        finish(&mut browser, &jobs);
        assert_eq!(browser.history.len(), 3);
        assert_eq!(browser.location().unwrap().kind, LocationKind::Folder);
        browser.history_step(false);
        assert!(browser.home_surface.open);
        browser.show_home();
        assert_eq!(
            browser.history.len(),
            3,
            "opening the current Home visit is idempotent"
        );
    }
    #[test]
    fn unavailable_return_context_keeps_home_visible_and_does_not_show_another_folder() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        std::fs::create_dir(&old).unwrap();
        let other = root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let (mut browser, jobs) = loaded(&old);
        browser.show_home();
        browser.navigate(other.clone(), Navigation::New);
        finish(&mut browser, &jobs);
        std::fs::remove_dir(&old).unwrap();
        browser.history_step(false);
        assert!(browser.home_surface.open);
        assert_eq!(browser.directory.as_ref().unwrap().path, other);
        browser.return_from_home();
        assert!(
            browser.home_surface.open,
            "pending return must not expose another folder's cache"
        );
        finish(&mut browser, &jobs);
        assert!(
            browser.home_surface.open,
            "failed context restore must not expose the unrelated retained listing"
        );
        assert_eq!(browser.index, Some(1));
        assert!(browser.error.is_some());
    }
}
