//! Depth-bounded traversal and transactional publication of directory coverage.
use super::*;

pub(super) struct Scanner<'a> {
    db: &'a Store,
    root: &'a Root,
    id: &'a str,
    limits: &'a Limits,
    cancel: &'a AtomicBool,
    stack: Vec<Frame>,
    pending: Vec<Entry>,
    started: Instant,
    count: u64,
    bytes: u64,
    payload: u64,
    complete: bool,
    messages: Vec<String>,
    directory: CatalogDirectoryCoverage,
    stopped: Option<CatalogScanIssue>,
}
impl<'a> Scanner<'a> {
    pub fn new(
        db: &'a Store,
        root: &'a Root,
        id: &'a str,
        limits: &'a Limits,
        cancel: &'a AtomicBool,
    ) -> Result<Self> {
        let mut scanner = Self {
            db,
            root,
            id,
            limits,
            cancel,
            stack: Vec::new(),
            pending: Vec::with_capacity(128),
            started: Instant::now(),
            count: 0,
            bytes: 0,
            payload: 0,
            complete: true,
            messages: Vec::new(),
            directory: CatalogDirectoryCoverage::default(),
            stopped: None,
        };
        match Frame::new(root, PathBuf::new()) {
            Ok(frame) => scanner.stack.push(frame),
            Err(error) if limits.detailed_coverage => {
                scanner.complete = false;
                scanner.directory.issue = Some(issue(CatalogScanStage::OpenDirectory, &error));
                scanner
                    .message("root directory could not be inventoried; see capture details".into());
            }
            Err(error) => return Err(error),
        }
        Ok(scanner)
    }
    pub fn run(mut self, mut progress: impl FnMut(u64, u64)) -> Result<Coverage> {
        let mut last_progress = 0;
        while !self.stack.is_empty() && self.stopped.is_none() {
            cancelled(self.cancel)?;
            self.step()?;
            if self.pending.len() == 128 {
                self.flush()?;
            }
            if self.count.saturating_sub(last_progress) >= 128 {
                self.root.validate()?;
                progress(self.count, self.bytes);
                last_progress = self.count;
            }
        }
        self.flush()?;
        self.root.validate()?;
        progress(self.count, self.bytes);
        Ok(self.report())
    }
    fn report(mut self) -> Coverage {
        if !self.stack.is_empty() {
            self.directory = self.stack[0].coverage.clone();
            self.directory.listing_complete = false;
            self.directory.subtree_complete = false;
            if let Some(stopped) = self.stopped {
                self.directory.issue.get_or_insert(stopped);
            }
        }
        let complete = self.complete && self.stopped.is_none();
        let message = if !complete {
            format!("Partial: {}", self.messages.join("; "))
        } else if self.limits.exclusions.is_empty() {
            "Complete".into()
        } else {
            "Complete with configured exclusions".into()
        };
        Coverage {
            count: self.count,
            complete,
            message,
            directory: self.directory,
            stopped: self.stopped,
        }
    }
    fn step(&mut self) -> Result<()> {
        let frame = self.stack.last_mut().expect("active directory");
        let Some(child) = frame.reader.next() else {
            return self.finish_directory(true);
        };
        if let Some(reason) = self.budget() {
            self.stop(reason, "catalog entry, byte or time budget reached");
            return Ok(());
        }
        match child {
            Ok(child) => self.collect(child.file_name()),
            Err(error) => {
                self.stack
                    .last_mut()
                    .expect("active directory")
                    .coverage
                    .issue = Some(issue(CatalogScanStage::ReadDirectory, &error.into()));
                self.complete = false;
                self.message("directory enumeration failed; see directory coverage".into());
                self.finish_directory(false)
            }
        }
    }
    fn budget(&self) -> Option<CatalogScanReason> {
        if self.count >= self.limits.entries {
            Some(CatalogScanReason::Entries)
        } else if self.bytes >= self.limits.bytes {
            Some(CatalogScanReason::Bytes)
        } else if self.started.elapsed() >= self.limits.timeout {
            Some(CatalogScanReason::Time)
        } else {
            None
        }
    }
    fn collect(&mut self, name: std::ffi::OsString) -> Result<()> {
        let relative = self
            .stack
            .last()
            .expect("active directory")
            .relative
            .join(name);
        let mut entry = match collect_entry(
            self.db,
            self.root,
            &relative,
            self.limits,
            self.limits.hash_bytes.saturating_sub(self.payload),
            self.cancel,
        ) {
            Ok(entry) => entry,
            Err(error) => {
                cancelled(self.cancel)?;
                unavailable(relative.clone(), &error, self.limits.detailed_coverage)
            }
        };
        if entry.kind == "file" && self.limits.hashes {
            self.payload = self.payload.saturating_add(entry.size);
        }
        let next = self.descend(&mut entry);
        let parent = &mut self.stack.last_mut().expect("active directory").coverage;
        if entry.warning.is_some() {
            parent.failed_entries += 1;
        }
        parent.incomplete_child |= entry.kind == "excluded" || entry.warning.is_some();
        self.complete &= entry.warning.is_none();
        self.bytes = self.bytes.saturating_add(
            (serde_json::to_string(&entry)?.len() + relative.as_os_str().as_bytes().len() * 3 + 256)
                as u64,
        );
        if self.bytes > self.limits.bytes {
            self.stop(CatalogScanReason::Bytes, "catalog byte budget reached");
            return Ok(());
        }
        if let Some(warning) = &entry.warning {
            self.message(if self.limits.detailed_coverage {
                "entry metadata or subtree unavailable; see saved entry diagnostics".into()
            } else {
                format!("{}: {warning}", relative.display())
            });
        }
        self.pending.push(entry);
        self.count += 1;
        if let Some(frame) = next {
            self.stack.push(frame);
        }
        Ok(())
    }
    fn descend(&self, entry: &mut Entry) -> Option<Frame> {
        if !matches!(entry.kind.as_str(), "directory" | "excluded") {
            return None;
        }
        let mut coverage = CatalogDirectoryCoverage::default();
        let next = if entry.kind == "excluded" {
            None
        } else if self.stack.len() >= self.limits.depth {
            entry.warning = Some("Directory depth limit".into());
            coverage.issue = Some(limit(CatalogScanReason::Depth));
            None
        } else {
            match Frame::new(self.root, entry.path.clone()) {
                Ok(frame) => Some(frame),
                Err(error) => {
                    entry.warning = Some(error.to_string());
                    coverage.issue = Some(issue(CatalogScanStage::OpenDirectory, &error));
                    None
                }
            }
        };
        if self.limits.detailed_coverage {
            entry.directory_coverage = Some(coverage);
        }
        next
    }
    fn finish_directory(&mut self, listing_complete: bool) -> Result<()> {
        // Coverage cannot claim completion before its child rows are committed.
        if self.limits.detailed_coverage {
            self.flush()?;
        }
        if self.stopped.is_some() {
            return Ok(());
        }
        let mut frame = self.stack.pop().expect("finished directory");
        frame.coverage.listing_complete = listing_complete;
        if self.limits.detailed_coverage
            && let Err(error) = frame.validate(self.root)
        {
            frame.coverage.listing_complete = false;
            frame
                .coverage
                .issue
                .get_or_insert(issue(CatalogScanStage::ReadDirectory, &error));
            self.complete = false;
            self.message("directory identity could not be confirmed after enumeration".into());
        }
        frame.coverage.subtree_complete = frame.coverage.listing_complete
            && frame.coverage.issue.is_none()
            && frame.coverage.failed_entries == 0
            && !frame.coverage.incomplete_child;
        if let Some(parent) = self.stack.last_mut() {
            parent.coverage.incomplete_child |= !frame.coverage.subtree_complete;
        }
        if frame.relative.as_os_str().is_empty() {
            self.directory = frame.coverage;
        } else if self.limits.detailed_coverage {
            self.commit_directory_coverage(&frame.relative, frame.coverage)?;
        }
        Ok(())
    }
    fn commit_directory_coverage(
        &mut self,
        path: &Path,
        coverage: CatalogDirectoryCoverage,
    ) -> Result<()> {
        let mut entry =
            super::entry(self.db, self.id, path)?.ok_or("Missing inventoried directory")?;
        entry.directory_coverage = Some(coverage);
        match put(self.db, self.id, &entry) {
            Ok(()) => (),
            Err(error) if database_full(&error) => self.stop(
                CatalogScanReason::Database,
                "catalog database byte limit reached",
            ),
            Err(error) => return Err(error),
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if !self.pending.is_empty() && !super::flush(self.db, self.id, &self.pending)? {
            self.stop(
                CatalogScanReason::Database,
                "catalog database byte limit reached",
            );
            self.count = stored_count(self.db, self.id)?;
        }
        self.pending.clear();
        Ok(())
    }
    fn stop(&mut self, reason: CatalogScanReason, message: &str) {
        self.stopped.get_or_insert(limit(reason));
        self.complete = false;
        self.message(message.into());
    }
    fn message(&mut self, message: String) {
        if self.messages.len() < 8 && !self.messages.contains(&message) {
            self.messages.push(message);
        }
    }
}
fn database_full(error: &crate::Error) -> bool {
    matches!(error, crate::Error::Sql(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::DiskFull)
}
