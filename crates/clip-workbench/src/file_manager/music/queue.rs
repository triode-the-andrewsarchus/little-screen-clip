use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::file_manager) enum Repeat {
    #[default]
    Off,
    All,
    One,
}
impl Repeat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
            Self::One => "one",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}
#[derive(Default)]
pub(in crate::file_manager) struct Queue {
    pub paths: Vec<PathBuf>,
    order: Vec<usize>,
    cursor: usize,
    pub repeat: Repeat,
    pub shuffled: bool,
}
impl Queue {
    pub fn current(&self) -> Option<&PathBuf> {
        self.order.get(self.cursor).and_then(|i| self.paths.get(*i))
    }
    pub fn replace(&mut self, paths: Vec<PathBuf>, path: &Path) -> bool {
        let Some(index) = paths.iter().position(|p| p == path) else {
            return false;
        };
        self.paths = paths;
        self.order = (0..self.paths.len()).collect();
        self.cursor = index;
        if self.shuffled {
            self.shuffle_from_current();
        }
        true
    }
    pub fn select(&mut self, index: usize) {
        if let Some(cursor) = self.order.iter().position(|i| *i == index) {
            self.cursor = cursor;
        }
    }
    pub fn toggle_shuffle(&mut self) {
        self.shuffled = !self.shuffled;
        if self.shuffled {
            self.shuffle_from_current();
        } else {
            let index = self.order.get(self.cursor).copied().unwrap_or(0);
            self.order = (0..self.paths.len()).collect();
            self.cursor = index;
        }
    }
    fn shuffle_from_current(&mut self) {
        let current = self.order.get(self.cursor).copied();
        let mut keyed: Vec<_> = self
            .order
            .iter()
            .map(|index| (uuid::Uuid::new_v4(), *index))
            .collect();
        keyed.sort_unstable();
        self.order = keyed.into_iter().map(|(_, index)| index).collect();
        if let Some(index) = current.and_then(|c| self.order.iter().position(|i| *i == c)) {
            self.order.swap(0, index);
        }
        self.cursor = 0;
    }
    pub fn advance(&mut self, forward: bool, manual: bool) -> bool {
        if self.order.is_empty() {
            return false;
        }
        if !manual && self.repeat == Repeat::One {
            return true;
        }
        if forward {
            if self.cursor + 1 < self.order.len() {
                self.cursor += 1;
            } else if self.repeat == Repeat::All || manual {
                self.cursor = 0;
            } else {
                return false;
            }
        } else {
            self.cursor = self.cursor.checked_sub(1).unwrap_or(self.order.len() - 1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_order_repeat_and_manual_next_are_explicit() {
        let mut queue = Queue::default();
        assert!(queue.replace(["a", "b", "c"].map(PathBuf::from).to_vec(), Path::new("a")));
        assert!(queue.advance(true, false));
        assert_eq!(queue.current().unwrap(), Path::new("b"));
        queue.repeat = Repeat::One;
        assert!(queue.advance(true, false));
        assert_eq!(queue.current().unwrap(), Path::new("b"));
        assert!(queue.advance(true, true));
        assert_eq!(queue.current().unwrap(), Path::new("c"));
        queue.repeat = Repeat::Off;
        assert!(!queue.advance(true, false));
        queue.repeat = Repeat::All;
        assert!(queue.advance(true, false));
        assert_eq!(queue.current().unwrap(), Path::new("a"));
    }
    #[test]
    fn shuffle_keeps_current_track_and_visits_each_entry_once() {
        let paths: Vec<_> = (0..1000).map(|i| PathBuf::from(i.to_string())).collect();
        let mut queue = Queue::default();
        queue.replace(paths, Path::new("500"));
        queue.toggle_shuffle();
        assert_eq!(queue.current().unwrap(), Path::new("500"));
        let mut visited = std::collections::BTreeSet::new();
        loop {
            visited.insert(queue.current().unwrap().clone());
            if !queue.advance(true, false) {
                break;
            }
        }
        assert_eq!(visited.len(), 1000);
        let current = queue.current().cloned();
        queue.toggle_shuffle();
        assert_eq!(queue.current().cloned(), current);
    }
}
