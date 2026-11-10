use super::*;

impl Worker {
    pub(super) fn persist_roots(&mut self) -> Result<(), String> {
        let roots = self
            .database()?
            .roots()?
            .iter()
            .map(|root| filesystem::display_path(&root.path))
            .collect::<Vec<_>>();
        little_clip_core::config::save_file_search_roots(&roots).map_err(|e| e.to_string())?;
        self.config.roots = roots;
        self.snapshot.lock().unwrap().settings = self.config.clone();
        Ok(())
    }
    pub(super) fn enrollment_id(&self, path: &Path) -> Result<Option<i64>, String> {
        let stored = self
            .store
            .as_ref()
            .map(|store| store.root_id(path))
            .transpose()?
            .flatten();
        Ok(stored.or_else(|| {
            self.work.iter().find_map(|work| match work {
                Work::Probe {
                    id, path: pending, ..
                } if pending == path => Some(*id),
                _ => None,
            })
        }))
    }
    pub(super) fn enroll(&mut self, path: &Path, persist: bool) -> Result<(), String> {
        if let Some(reason) = super::super::policy::excluded(path) {
            return Err(reason.into());
        }
        if super::super::policy::excluded_by_user(path, &self.exclusions) {
            return Err("Folder is excluded by search preferences".into());
        }
        if let Some(Work::Probe {
            persist: pending, ..
        }) = self
            .work
            .iter_mut()
            .find(|work| matches!(work, Work::Probe { path: pending, .. } if pending == path))
        {
            *pending |= persist;
            return Ok(());
        }
        if self.work.len() >= 100_000 {
            return Err("Index work queue is full; reconciliation will retry".into());
        }
        let stored = self.enrollment_id(path)?;
        let id = match stored {
            Some(id) if id > 0 => {
                // A pending source recheck must not authorize cached results.
                self.store
                    .as_ref()
                    .unwrap()
                    .invalidate_directory(id, None)?;
                id
            }
            Some(_) => return Err("Invalid stored root identifier".into()),
            None => {
                self.next_probe = self
                    .next_probe
                    .checked_sub(1)
                    .ok_or("Enrollment request IDs exhausted")?;
                self.next_probe
            }
        };
        self.cancel_reads(Some(id));
        self.work.retain(|work| work.root_id() != id);
        self.work.push_back(Work::Probe {
            id,
            path: path.into(),
            persist,
        });
        self.update_roots(true);
        Ok(())
    }
    pub(super) fn probe(&mut self, id: i64, path: PathBuf, persist: bool) -> Result<(), String> {
        let result = self
            .readers
            .request(id, 0, &path, Request::Probe(path.clone()));
        let result = match result {
            Ok(None) => {
                self.work.push_back(Work::Probe { id, path, persist });
                return Ok(());
            }
            Ok(Some(Reply::Root(root))) => self.start_scan(root, persist),
            Ok(Some(_)) => Err("Invalid enrollment reply".into()),
            Err(error) => Err(error),
        };
        if let Err(error) = &result
            && let Some(store) = &self.store
        {
            match store.unavailable(&path, error) {
                Ok(Some(id)) => {
                    self.cancel_reads(Some(id));
                    self.work.retain(|work| work.root_id() != id);
                    self.monitor.failed(id);
                }
                Ok(None) => {}
                Err(error) => self.report(Err(error)),
            }
        }
        result
    }
    pub(super) fn start_scan(&mut self, root: SourceRoot, persist: bool) -> Result<(), String> {
        self.monitor.check_source(&root)?;
        if root.path.starts_with(&self.paths.data) {
            return Err("The search cache cannot index itself".into());
        }
        if super::super::policy::excluded_by_user(&root.path, &self.exclusions) {
            return Err("Folder is excluded by search preferences".into());
        }
        let id = if persist {
            self.database()?.enroll_new_source(&root)?
        } else {
            self.database()?.enroll_source(&root)?
        };
        if persist {
            self.persist_roots()?;
        }
        self.cancel_reads(Some(id));
        self.work.retain(|work| work.root_id() != id);
        let generation = self.database()?.begin_scan(id)?;
        self.monitor.started(id, root.clone());
        self.work.push_back(Work::Directory {
            path: root.path.clone(),
            root,
            id,
            generation,
            recursive: true,
            remaining: None,
        });
        self.update_roots(true);
        Ok(())
    }
    pub(super) fn step(&mut self, work: Work) -> Result<(), String> {
        match work {
            Work::Probe { id, path, persist } => self.probe(id, path, persist),
            Work::Directory {
                root,
                id,
                generation,
                path,
                recursive,
                remaining,
            } => {
                // Both inventory and retirement obey the current policy.
                if self.index_excludes(&path, &root.path) {
                    self.database()?.retire_path(id, &path)
                } else if let Some(remaining) = remaining {
                    self.finish_directory_inventory(root, id, generation, path, remaining)
                } else {
                    self.scan(root, id, generation, path, recursive)
                }
            }
            Work::Inventory(inventory) => self.apply_inventory(*inventory),
            Work::File(file) => self.index_file(*file),
        }
    }
    pub(super) fn index_excludes(&self, path: &Path, root: &Path) -> bool {
        path.starts_with(&self.paths.data)
            || super::super::policy::excluded_by_user(path, &self.exclusions)
            || super::super::policy::excluded_from_root(path, root, self.config.include_hidden)
    }

    pub(super) fn scan(
        &mut self,
        root: SourceRoot,
        id: i64,
        generation: i64,
        path: PathBuf,
        recursive: bool,
    ) -> Result<(), String> {
        let Some(inventory) = self.read_inventory(&root, id, generation, &path)? else {
            self.work.push_back(Work::Directory {
                root,
                id,
                generation,
                path,
                recursive,
                remaining: None,
            });
            return Ok(());
        };
        // Identify the pipeline before admitting current segment reuse.
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        if let Err(error) = self.model(Role::Embedding, &stop) {
            if self.background_interrupted(&error) {
                self.work.push_front(Work::Directory {
                    root,
                    id,
                    generation,
                    path,
                    recursive,
                    remaining: None,
                });
                return Ok(());
            }
            return Err(error);
        }
        self.work
            .push_front(Work::Inventory(Box::new(InventoryWork {
                invalidated: false,
                remaining_bytes: inventory.storage_bytes,
                _permit: inventory.permit,
                entries: inventory.directory.entries.into(),
                pipeline: self.pipeline.clone(),
                root,
                id,
                generation,
                path,
                recursive,
            })));
        Ok(())
    }
    pub(super) fn apply_inventory(&mut self, mut inventory: InventoryWork) -> Result<(), String> {
        let InventoryWork { id, generation, .. } = inventory;
        if self.index_excludes(&inventory.path, &inventory.root.path) {
            return self.database()?.retire_path(id, &inventory.path);
        }
        if inventory.invalidated {
            // A partial application must not restore membership from an old
            // snapshot after an event. A new generation also revisits removals
            // among entries already observed in this abandoned application.
            let generation = self.database()?.begin_delta(id)?;
            // Pending files from this abandoned directory will be reconciled by
            // the fresh inventory. Their committed checkpoints remain reusable.
            self.work.retain(|work| {
                !matches!(work, Work::File(file)
                if file.root_id == id && file.entry.path.parent() == Some(inventory.path.as_path()))
            });
            self.work.push_front(Work::Directory {
                root: inventory.root,
                id,
                generation,
                path: inventory.path,
                recursive: inventory.recursive,
                remaining: None,
            });
            return Ok(());
        }
        self.observe_inventory(&mut inventory)?;
        if !inventory.entries.is_empty() {
            self.work.push_back(Work::Inventory(Box::new(inventory)));
            return Ok(());
        }
        let remaining = self
            .database()?
            .unseen_children(id, generation, &inventory.path)?;
        self.finish_directory_inventory(
            inventory.root,
            id,
            generation,
            inventory.path,
            remaining.into(),
        )
    }
    pub(super) fn observe_inventory(
        &mut self,
        inventory: &mut InventoryWork,
    ) -> Result<(), String> {
        let started = Instant::now();
        let before = inventory.entries.len();
        let cancel = self.dispatch.lock().unwrap().cancel.clone();
        for _ in 0..64 {
            if cancel.load(Ordering::Relaxed) || started.elapsed() >= Duration::from_millis(8) {
                break;
            }
            let Some(entry) = inventory.entries.pop_front() else {
                break;
            };
            inventory.remaining_bytes = inventory
                .remaining_bytes
                .saturating_sub(reader::Inventory::entry_storage_bytes(&entry)?);
            if self.index_excludes(&entry.path, &inventory.root.path) {
                continue;
            }
            if entry.directory
                && !inventory.recursive
                && self
                    .database()?
                    .known_directory(inventory.id, &entry.path)?
            {
                continue;
            }
            self.observe_entry(
                &inventory.root,
                inventory.id,
                inventory.generation,
                entry,
                &inventory.pipeline,
            )?;
        }
        tracing::debug!(target: "little_clip_search_timing",
            rows = before - inventory.entries.len(), remaining = inventory.entries.len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "directory inventory batch");
        Ok(())
    }
    pub(super) fn read_inventory(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        path: &Path,
    ) -> Result<Option<reader::Inventory>, String> {
        self.snapshot.lock().unwrap().message =
            format!("Scanning {}", filesystem::display_path(path));
        match self.watch_directory(root, id, generation, path) {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => {
                self.cancel_reads(Some(id));
                self.monitor.failed(id);
                self.snapshot.lock().unwrap().error = Some(format!(
                    "Change monitoring incomplete: {error}; periodic reconciliation remains enabled"
                ));
            }
        }
        let Some(reply) = self.readers.request(
            id,
            generation,
            path,
            Request::List {
                output: None,
                root: root.clone(),
                path: path.into(),
                hidden: self.config.include_hidden,
                exclusions: self.exclusions.clone(),
            },
        )?
        else {
            return Ok(None);
        };
        let Reply::Directory(inventory) = reply else {
            return Err("Invalid directory reply".into());
        };
        if inventory.directory.skipped > 0 {
            return Err("Directory enumeration incomplete; existing inventory was retained".into());
        }
        // The worker reserves the complete inventory before any row is written.
        if !self.admit(None, 0, inventory.storage_bytes)? {
            return Ok(None);
        }
        Ok(Some(inventory))
    }
    pub(super) fn watch_directory(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        path: &Path,
    ) -> Result<bool, String> {
        let instance = match self.readers.watch_handle(id) {
            Some(instance) => instance,
            None if self.readers.can_start(id) => {
                self.monitor.prepare_watch(self.readers.watch_count())?
            }
            None => return Ok(true), // An inventory request is already pending.
        };
        let reply = match self
            .readers
            .watch(id, generation, path, root.clone(), instance.clone())
        {
            Ok(None) => return Ok(false),
            Ok(Some(registration)) => Ok(registration),
            Err(error) => Err(error),
        };
        self.monitor.complete_watch(id, path, &instance, reply)?;
        Ok(true)
    }
    pub(super) fn finish_directory_inventory(
        &mut self,
        root: SourceRoot,
        id: i64,
        generation: i64,
        path: PathBuf,
        mut remaining: VecDeque<PathBuf>,
    ) -> Result<(), String> {
        if let Some(unseen) = remaining.pop_front() {
            let excluded = self.index_excludes(&unseen, &root.path);
            // A policy change retires generated references even when the source
            // still exists. Ordinary removals continue to require absence proof.
            let absent = if excluded {
                true
            } else {
                match self.readers.request(
                    id,
                    generation,
                    &path,
                    Request::Absent {
                        root: root.clone(),
                        path: unseen.clone(),
                    },
                ) {
                    Ok(Some(Reply::Verified)) => true,
                    Ok(None) => {
                        remaining.push_front(unseen.clone());
                        false
                    }
                    _ => false,
                }
            };
            if absent {
                self.database()?.retire_path(id, &unseen)?;
            }
        }
        if remaining.is_empty() {
            self.database()?.directory_complete(id, generation, &path)
        } else {
            self.work.push_front(Work::Directory {
                root,
                id,
                generation,
                path,
                recursive: false,
                remaining: Some(remaining),
            });
            Ok(())
        }
    }
    pub(super) fn observe_entry(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        entry: Entry,
        pipeline: &str,
    ) -> Result<(), String> {
        // Reserve one slot for this directory's retirement continuation.
        if self.work.len() >= 99_999 {
            return Err(
                "Index work queue reached its 100,000-entry bound; coverage is incomplete".into(),
            );
        }
        if entry.directory {
            self.work.push_back(Work::Directory {
                root: root.clone(),
                id,
                generation,
                path: entry.path,
                recursive: true,
                remaining: None,
            });
        } else if let Some(object) = entry.identity.map(|id| id.object()) {
            let (file_id, pending) = self
                .database()?
                .observe_object(id, generation, &entry, object, pipeline)?;
            if pending {
                let file = FileWork::new(root.clone(), id, generation, entry, file_id);
                if self.retry_ready(&file)? {
                    self.work.push_back(Work::File(Box::new(file)));
                }
            }
        }
        Ok(())
    }
    pub(super) fn index_file(&mut self, mut file: FileWork) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("Search cancelled".into());
        }
        if self.index_excludes(&file.entry.path, &file.root.path) {
            return self.database()?.retire_path(file.root_id, &file.entry.path);
        }
        if !file.publishing && file.segments.is_empty() && !self.retry_ready(&file)? {
            return Ok(());
        }
        if file.publishing {
            return self.publish_file(file);
        }
        let pipeline = self.pipeline.clone();
        if file.segments.is_empty()
            && self
                .database()?
                .file_complete(file.id, file.entry.revision, &pipeline)?
        {
            if !self.verify_index_file(&file)? {
                self.work.push_back(Work::File(Box::new(file)));
            }
            return Ok(());
        }
        if std::mem::take(&mut file.resume_pending) {
            self.resume_file(&mut file)?;
        }
        let resume = matches!(file.source_check, SourceCheck::Pending);
        let ready = match self.check_file_content(&mut file) {
            Ok(ready) => ready,
            Err(error) => return self.file_failure(file, error),
        };
        if resume && ready {
            self.resume_file(&mut file)?;
        }
        if !ready || !self.checkpoint_file(&mut file)? {
            self.work.push_back(Work::File(Box::new(file)));
            return Ok(());
        }
        if matches!(file.source_check, SourceCheck::Reuse(_)) {
            if !self.admit(Some(&file), 0, 0)? {
                self.work.push_front(Work::File(Box::new(file)));
                return Ok(());
            }
            return self.publish_file(file);
        }
        self.extract_file(file)
    }
    pub(super) fn reserve_file_staging(&mut self, file: &mut FileWork) -> bool {
        if file.staging.is_some() {
            return true;
        }
        self.staged_files.retain(|slot| slot.strong_count() != 0);
        if self.staged_files.len() == MAX_STAGED_FILES
            || self
                .staged_files
                .iter()
                .filter_map(Weak::upgrade)
                .any(|owner| *owner == file.id)
        {
            return false;
        }
        let permit = Arc::new(file.id);
        self.staged_files.push(Arc::downgrade(&permit));
        file.staging = Some(permit);
        true
    }
    pub(super) fn extract_file(&mut self, mut file: FileWork) -> Result<(), String> {
        if file.segments.len() >= MAX_FILE_SEGMENTS {
            file.coverage = format!(
                "partial: segment limit {MAX_FILE_SEGMENTS}; {}",
                file.coverage
            );
            return self.publish_file(file);
        }
        self.snapshot.lock().unwrap().message = format!("Indexing {}", file.entry.name);
        if file.prepared.is_none() {
            let result = self.read_prepared(&mut file);
            let prepared = match result {
                Ok(Some(prepared)) => prepared,
                Ok(None) => {
                    self.work.push_back(Work::File(Box::new(file)));
                    return Ok(());
                }
                Err(error) => return self.file_failure(file, error),
            };
            file.coverage = prepared.coverage.clone();
            file.prepared = Some(prepared);
        }
        let prepared = file.prepared.as_ref().unwrap();
        if file.text_cursor < prepared.text.len() {
            if !self.admit(Some(&file), 1, 0)? {
                self.work.push_front(Work::File(Box::new(file)));
                return Ok(());
            }
            let segment = match self.text_segment(&mut file) {
                Ok(segment) => segment,
                Err(error) => {
                    self.model = None;
                    self.snapshot.lock().unwrap().backend = None;
                    return self.file_failure(file, error);
                }
            };
            file.segments.push(segment);
            self.work.push_front(Work::File(Box::new(file)));
            return Ok(());
        }
        if prepared.data.is_some() && !self.admit(Some(&file), 1, 0)? {
            self.work.push_front(Work::File(Box::new(file)));
            return Ok(());
        }
        let prepared = file.prepared.take().unwrap();
        self.finish_file(file, prepared)
    }
    pub(super) fn check_file_content(&mut self, file: &mut FileWork) -> Result<bool, String> {
        if !matches!(file.source_check, SourceCheck::Pending) {
            return Ok(true);
        }
        let digest = match self.read_file(
            file,
            Request::Fingerprint {
                root: file.root.clone(),
                entry: file.entry.clone(),
            },
        )? {
            None => return Ok(false),
            Some(Reply::Fingerprint(digest)) => digest,
            _ => return Err("Invalid source fingerprint reply".into()),
        };
        file.source_check = SourceCheck::Extract(digest);
        if let Some(digest) = digest {
            let pipeline = self.pipeline.clone();
            if let Some((segments, coverage)) =
                self.database()?
                    .reusable_source(&file.entry, &pipeline, &digest)?
            {
                file.segments = segments;
                file.coverage = coverage;
                file.source_check = SourceCheck::Reuse(digest);
            }
        }
        Ok(true)
    }
    pub(super) fn text_segment(&mut self, file: &mut FileWork) -> Result<Segment, String> {
        let prepared = file.prepared.as_ref().unwrap();
        let start = file.text_cursor;
        let mut limit = (start + 8192).min(prepared.text.len());
        while !prepared.text.is_char_boundary(limit) {
            limit -= 1;
        }
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        let (end, next) = self
            .model(Role::Embedding, &stop)?
            .passage(&prepared.text[start..limit], &stop)?;
        let text = prepared.text[start..start + end].to_owned();
        let next_cursor = if start + end == prepared.text.len() {
            start + end
        } else {
            start + next
        };
        let location = format!(
            "{} · UTF-8 bytes {}..{}",
            prepared.location,
            start,
            start + end
        );
        let input = format!("title: none | text: {text}");
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        let input_hash = Sha256::digest(input.as_bytes()).to_vec();
        let vector = match self.cached_vector("text", &input_hash)? {
            Some(vector) => vector,
            None => {
                let vector = self
                    .model(Role::Embedding, &stop)?
                    .texts(std::slice::from_ref(&input), false, &stop)?
                    .remove(0);
                self.snapshot.lock().unwrap().generated_vectors += 1;
                vector
            }
        };
        // Commit the cursor only together with a complete vector. Preemption
        // must retry this passage rather than silently dropping its text.
        file.text_cursor = next_cursor;
        Ok(Segment {
            modality: "text".into(),
            location,
            text,
            input_hash,
            vector: Some(vector),
        })
    }
    pub(super) fn cached_vector(
        &mut self,
        modality: &str,
        hash: &[u8],
    ) -> Result<Option<Vec<f32>>, String> {
        let pipeline = self.pipeline.clone();
        let vector = self
            .database()?
            .reusable_vector(&pipeline, modality, hash)?;
        if vector.is_some() {
            self.snapshot.lock().unwrap().reused_vectors += 1;
        }
        Ok(vector)
    }

    pub(super) fn media_segment(&mut self, prepared: &Prepared) -> Result<Segment, String> {
        let bytes = STANDARD
            .decode(prepared.data.as_ref().unwrap())
            .map_err(|e| e.to_string())?;
        let budget = prepared.vision_tokens.unwrap_or(280);
        if !matches!(budget, 140 | 280) {
            return Err("Unsupported vision token budget".into());
        }
        let input_hash = media_hash(&bytes, &prepared.modality, budget);
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        let vector = match self.cached_vector(&prepared.modality, &input_hash)? {
            Some(vector) => vector,
            None => {
                let vector = self.model(Role::Embedding, &stop)?.media(
                    &bytes,
                    prepared.modality == "image",
                    budget,
                    &stop,
                )?;
                self.snapshot.lock().unwrap().generated_vectors += 1;
                vector
            }
        };
        Ok(Segment {
            modality: prepared.modality.clone(),
            location: prepared.location.clone(),
            text: String::new(),
            input_hash,
            vector: Some(vector),
        })
    }
    pub(super) fn finish_file(
        &mut self,
        mut file: FileWork,
        prepared: Prepared,
    ) -> Result<(), String> {
        if prepared.data.is_some() {
            match self.media_segment(&prepared) {
                Ok(segment) => file.segments.push(segment),
                Err(error) => {
                    self.model = None;
                    self.snapshot.lock().unwrap().backend = None;
                    if self.background_interrupted(&error) {
                        file.prepared = Some(prepared);
                    }
                    return self.file_failure(file, error);
                }
            }
        }
        if let Some(next) = prepared.next {
            file.cursor = next;
            file.text_cursor = 0;
            self.work.push_front(Work::File(Box::new(file)));
            Ok(())
        } else {
            self.publish_file(file)
        }
    }
    pub(super) fn retry_ready(&mut self, file: &FileWork) -> Result<bool, String> {
        match self.database()?.file_retry(file.id)? {
            super::super::store::Retry::Ready => Ok(true),
            super::super::store::Retry::After(delay) => {
                if let Some(parent) = file.entry.path.parent() {
                    self.monitor.retry_directory(file.root_id, parent, delay);
                }
                Ok(false)
            }
            super::super::store::Retry::Stopped => Ok(false),
        }
    }
    pub(super) fn file_failure(&mut self, mut file: FileWork, error: String) -> Result<(), String> {
        // A stopped reader/model cannot validate or publish a partial result.
        // Keep its last committed state, while retaining genuine failures for
        // the root error handler instead of converting them to cancellation.
        if self.stop.load(Ordering::Relaxed) {
            return Err(error);
        }
        if self.background_interrupted(&error) {
            self.work.push_front(Work::File(Box::new(file)));
            return Ok(());
        }
        let pipeline = self.pipeline.clone();
        if !self.database()?.record_file_failure(
            file.id,
            file.entry.revision,
            &pipeline,
            &error,
            !file.segments.is_empty(),
        )? {
            return Ok(());
        }
        self.retry_ready(&file)?;
        file.failed = true;
        if !file.segments.is_empty() {
            file.coverage = format!(
                "partial: {}; retained {} completed segments; planned coverage: {}",
                error.chars().take(240).collect::<String>(),
                file.segments.len(),
                file.coverage
            );
            let published = self.publish_file(file);
            return match published {
                Err(cancelled) if self.shutdown_cancelled(&cancelled) => Err(error),
                result => result,
            };
        }
        Ok(())
    }
    pub(super) fn background_interrupted(&self, error: &str) -> bool {
        let interrupted = self.dispatch.lock().unwrap().cancel.load(Ordering::Relaxed)
            && matches!(
                error,
                "Model loading cancelled" | "Search model request cancelled"
            );
        if interrupted {
            self.snapshot.lock().unwrap().backend = None;
        }
        interrupted
    }
    pub(super) fn shutdown_cancelled(&self, error: &str) -> bool {
        self.stop.load(Ordering::Relaxed)
            && matches!(
                error,
                "command cancelled"
                    | "Search cancelled"
                    | "Model loading cancelled"
                    | "Search model request cancelled"
            )
    }
    pub(super) fn publish_file(&mut self, mut file: FileWork) -> Result<(), String> {
        file.publishing = true;
        if !self.verify_index_file(&file)? {
            self.work.push_back(Work::File(Box::new(file)));
            return Ok(());
        }
        let pipeline = self.pipeline.clone();
        let published = self.database()?.publish(
            file.id,
            file.entry.revision,
            &pipeline,
            &file.segments,
            &file.coverage,
        )?;
        if published {
            if !file.failed {
                self.database()?.clear_file_retry(file.id)?;
            }
            if matches!(file.source_check, SourceCheck::Reuse(_)) {
                self.snapshot.lock().unwrap().reused_vectors += file
                    .segments
                    .iter()
                    .filter(|segment| segment.vector.is_some())
                    .count() as u64;
            }
            let digest = match file.source_check {
                SourceCheck::Pending => None,
                SourceCheck::Extract(digest) => digest,
                SourceCheck::Reuse(digest) => Some(digest),
            };
            if let Some(digest) = digest {
                self.database()?.remember_source(
                    file.id,
                    file.entry.revision,
                    &pipeline,
                    &file.entry,
                    &digest,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn read_prepared(
        &mut self,
        file: &mut FileWork,
    ) -> Result<Option<Prepared>, String> {
        let Some(reply) = self.read_file(
            file,
            Request::Extract {
                root: file.root.clone(),
                entry: file.entry.clone(),
                cursor: file.cursor,
                text_only: false,
            },
        )?
        else {
            return Ok(None);
        };
        match reply {
            Reply::Prepared(prepared) => {
                if let Some(expected) = file.resume_hash.take()
                    && (Sha256::digest(prepared.text.as_bytes()).as_slice() != expected
                        || !prepared.text.is_char_boundary(file.text_cursor))
                {
                    // A decoder's output is not an authoritative saved cursor.
                    // Re-extract and reuse only freshly matched canonical inputs.
                    file.cursor = 0;
                    file.text_cursor = 0;
                    file.published = 0;
                    file.segments.clear();
                    let pipeline = self.pipeline.clone();
                    self.database()?
                        .discard_checkpoint(file.id, file.entry.revision, &pipeline)?;
                    return Ok(None);
                }
                Ok(Some(prepared))
            }
            _ => Err("Invalid extraction reply".into()),
        }
    }
    pub(super) fn verify_index_file(&mut self, file: &FileWork) -> Result<bool, String> {
        match self.read_file(
            file,
            Request::Verify {
                root: file.root.clone(),
                entry: file.entry.clone(),
            },
        )? {
            None => Ok(false),
            Some(Reply::Verified) => Ok(true),
            _ => Err("Invalid source verification reply".into()),
        }
    }
    pub(super) fn read_file(
        &mut self,
        file: &FileWork,
        request: Request,
    ) -> Result<Option<Reply>, String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("Search cancelled".into());
        }
        self.readers
            .request(file.root_id, file.generation, &file.entry.path, request)
    }
    pub(super) fn resume_file(&mut self, file: &mut FileWork) -> Result<(), String> {
        let SourceCheck::Extract(digest) = file.source_check else {
            return Ok(());
        };
        let pipeline = self.pipeline.clone();
        if let Some((progress, segments)) = self.database()?.resume_file(
            file.id,
            file.entry.revision,
            &pipeline,
            &file.entry,
            digest,
        )? {
            file.cursor = progress.cursor;
            file.text_cursor = progress.text_cursor;
            file.coverage = progress.coverage;
            file.published = progress.segments;
            file.resume_hash = progress.prepared_hash;
            file.segments = segments;
            tracing::debug!(target: "little_clip_search_timing", segments = file.published,
                "resumed verified indexing checkpoint");
        }
        Ok(())
    }
    pub(super) fn checkpoint_file(&mut self, file: &mut FileWork) -> Result<bool, String> {
        if !matches!(file.source_check, SourceCheck::Extract(_))
            || file.segments.len().saturating_sub(file.published) < 8
        {
            return Ok(true);
        }
        if !self.verify_index_file(file)? {
            return Ok(false);
        }
        let digest = match file.source_check {
            SourceCheck::Extract(digest) => digest,
            _ => return Err("Checkpoint requires a verified extracted source".into()),
        };
        let progress = Checkpoint {
            cursor: file.cursor,
            text_cursor: file.text_cursor,
            coverage: file.coverage.clone(),
            format: super::super::store::source_format(&file.entry),
            digest,
            prepared_hash: file
                .prepared
                .as_ref()
                .filter(|_| file.text_cursor > 0)
                .map(|p| Sha256::digest(p.text.as_bytes()).into()),
            segments: file.published,
        };
        let pipeline = self.pipeline.clone();
        if !self.database()?.checkpoint(
            file.id,
            file.entry.revision,
            &pipeline,
            &file.segments,
            &progress,
        )? {
            return Err("Index source changed before checkpoint publication".into());
        }
        file.published = file.segments.len();
        self.yield_file_quantum(file);
        Ok(false)
    }
    pub(super) fn yield_file_quantum(&mut self, file: &mut FileWork) {
        // Move the next root ahead of this root's remaining jobs before the
        // caller appends this yielded file. The deque then preserves the next
        // root's turn rather than repeatedly favoring the first two roots.
        if let Some(next) = self
            .work
            .iter()
            .position(|work| work.root_id() != file.root_id)
        {
            self.work.rotate_left(next);
        }
        let waiting_for_staging = self.work.iter().any(
            |work| matches!(work, Work::File(next) if next.id != file.id && !next.publishing && next.staging.is_none()),
        );
        if waiting_for_staging
            && self
                .staged_files
                .iter()
                .filter(|slot| slot.strong_count() != 0)
                .count()
                == MAX_STAGED_FILES
        {
            // The committed cursor/hash can rebuild this exact prefix. Keep
            // the verified observation but release all prepared/cached bytes
            // before allowing another file to consume its staging allowance.
            file.prepared = None;
            file.segments = Vec::new();
            file.cursor = 0;
            file.text_cursor = 0;
            file.published = 0;
            file.resume_hash = None;
            file.resume_pending = true;
            file.staging = None;
        }
    }
    pub(super) fn search(
        &mut self,
        id: u64,
        path: &Path,
        query: &Query,
        hidden: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::super::Results, String> {
        match &query.operator {
            Operator::Embedding(text) => self.semantic(id, path, query, text, hidden, cancel),
            Operator::Name | Operator::Regex(_) | Operator::Content(_) => {
                self.on_demand(path, query, hidden, cancel)
            }
            Operator::Answer(_) => {
                Err("Answer commands create session columns rather than search results".into())
            }
        }
    }
    pub(super) fn on_demand(
        &mut self,
        path: &Path,
        query: &Query,
        hidden: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::super::Results, String> {
        let configured = self
            .config
            .roots
            .iter()
            .filter_map(|address| filesystem::decode_path(address).ok())
            .collect::<std::collections::HashSet<_>>();
        let mut roots = if query.scope == Scope::Indexed && configured.is_empty() {
            Vec::new()
        } else if query.scope == Scope::Indexed {
            self.database()?
                .monitor_roots()?
                .into_iter()
                .filter(|(_, root)| configured.contains(&root.path))
                .map(|(_, root)| {
                    super::super::walk::ScopeRoot::Indexed(self.monitor.bind_source(root))
                })
                .collect()
        } else {
            vec![super::super::walk::ScopeRoot::Folder(path.into())]
        };
        if query.progressive && query.scope == Scope::Recursive {
            roots.extend(self.config.always_search.iter().filter_map(|address| {
                filesystem::decode_path(address)
                    .ok()
                    .filter(|path| path.is_absolute())
                    .map(super::super::walk::ScopeRoot::Folder)
            }));
        }
        super::super::walk::run(
            roots,
            &self.paths.data,
            self.store.as_ref(),
            &self.exclusions,
            query,
            hidden,
            cancel,
        )
    }
    pub(super) fn semantic(
        &mut self,
        id: u64,
        path: &Path,
        query: &Query,
        text: &str,
        hidden: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::super::Results, String> {
        if self.config.roots.is_empty() && query.scope == Scope::Indexed {
            return Ok(super::super::Results {
                entries: Vec::new(),
                sources: Vec::new(),
                explanations: Default::default(),
                message: "No folders enrolled. Click Index to enable content and semantic search."
                    .into(),
            });
        }
        let current_enrolled = self
            .config
            .roots
            .iter()
            .filter_map(|address| filesystem::decode_path(address).ok())
            .any(|root| path.starts_with(root));
        if self.config.roots.is_empty() || (query.scope == Scope::Here && !current_enrolled) {
            let mut names = query.clone();
            names.operator = Operator::Name;
            names.name = text.into();
            let mut result = self.on_demand(path, &names, hidden, cancel)?;
            let coverage = if self.config.roots.is_empty() {
                "no folders enrolled"
            } else {
                "current folder not enrolled"
            };
            result.message = format!(
                "Filename results only · {coverage}. Click Index to enable content and semantic search."
            );
            return Ok(result);
        }
        let model_resident = self
            .model
            .as_ref()
            .is_some_and(|model| model.role == Role::Embedding);
        tracing::debug!(target: "little_clip_search_timing",
            request_id = id, model_resident, preliminary_lexical = !model_resident,
            "semantic search scheduling");
        // Cold loads still publish usable lexical results before loading the
        // model. A resident helper goes straight to the full fused pass, which
        // repeats all eligibility/source checks; don't scan and verify twice.
        if !model_resident {
            let mut lexical = self.ranked_results(path, query, text, hidden, &[], cancel)?;
            lexical.message = lexical
                .message
                .replace("Semantic candidates", "Lexical candidates")
                .replace(
                    "hybrid filename/keyword/meaning",
                    "filename/keyword · loading semantic model",
                );
            publish_search_result(
                &self.snapshot,
                SearchResult {
                    id,
                    complete: false,
                    result: Ok(lexical),
                },
            );
        }
        let input = format!("task: search result | query: {text}");
        let vector = self
            .model(Role::Embedding, cancel)
            .and_then(|model| model.texts(&[input], true, cancel));
        match vector {
            Ok(mut vectors) => {
                self.snapshot.lock().unwrap().progress = Some(SearchProgress {
                    id,
                    checked: Arc::new(AtomicU64::new(0)),
                });
                self.ranked_results(path, query, text, hidden, &vectors.remove(0), cancel)
            }
            Err(error) if !cancel.load(Ordering::Relaxed) => {
                self.model = None;
                self.snapshot.lock().unwrap().backend = None;
                // Model loading may have outlived a source edit. Refresh the
                // fallback instead of returning the pre-load verification.
                let mut lexical = self.ranked_results(path, query, text, hidden, &[], cancel)?;
                lexical.message = lexical
                    .message
                    .replace("Semantic candidates", "Lexical candidates")
                    .replace(
                        "hybrid filename/keyword/meaning",
                        "filename/keyword · semantic model unavailable",
                    );
                lexical.message.push_str(&format!(" · {error}"));
                Ok(lexical)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn ranked_results(
        &mut self,
        path: &Path,
        query: &Query,
        text: &str,
        hidden: bool,
        vector: &[f32],
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::super::Results, String> {
        let started = Instant::now();
        // Lexical text is reusable across model versions, but only from current
        // extraction records. A vector always requires its exact model pipeline.
        let pipeline = if vector.is_empty() {
            String::new()
        } else {
            self.pipeline.clone()
        };
        let floors = self.config.semantic_floors.clone();
        let exclusions = self.exclusions.clone();
        let index_hidden = self.config.include_hidden;
        let show_offline = self.config.show_offline_results;
        let roots = self
            .database()?
            .source_roots()?
            .into_iter()
            .map(|root| self.monitor.bind_source(root))
            .collect::<Vec<_>>();
        let progress = self
            .snapshot
            .lock()
            .unwrap()
            .progress
            .as_ref()
            .filter(|_| !vector.is_empty())
            .map(|progress| progress.checked.clone());
        let store = self.database()?;
        let retrieval_started = Instant::now();
        let hits = super::super::retrieval::Retrieval {
            progress: progress.as_deref(),
            store,
            pipeline: &pipeline,
            vector,
            path,
            query,
            text,
            hidden,
            exclusions: &exclusions,
            index_hidden,
            show_offline,
            floors: &floors,
            cancel,
        }
        .search()?;
        let retrieval_ms = retrieval_started.elapsed().as_secs_f64() * 1000.0;
        let sources = roots.clone();
        let ranked = hits.iter().map(|hit| hit.entry.clone()).collect::<Vec<_>>();
        let offline = if show_offline {
            store.last_known_paths(&ranked)?
        } else {
            Default::default()
        };
        let live = ranked
            .into_iter()
            .filter(|entry| !offline.contains(&entry.path))
            .collect();
        let verification_started = Instant::now();
        let (mut entries, mut message) = verify_candidates(live, roots, cancel)?;
        let verification_ms = verification_started.elapsed().as_secs_f64() * 1000.0;
        // Preserve fused order while admitting explicitly labelled last-known
        // records. A stale candidate in an available root never takes this path.
        entries.extend(
            hits.iter()
                .filter(|hit| offline.contains(&hit.entry.path))
                .map(|hit| hit.entry.clone()),
        );
        let order = hits
            .iter()
            .enumerate()
            .map(|(i, hit)| (hit.entry.path.clone(), i))
            .collect::<std::collections::HashMap<_, _>>();
        entries.sort_by_key(|entry| order[&entry.path]);
        let evidence_started = Instant::now();
        let mut explanations =
            super::super::explanation::read(store, &hits, &entries, &pipeline, text, cancel)?;
        let evidence_ms = evidence_started.elapsed().as_secs_f64() * 1000.0;
        for path in &offline {
            explanations.entry(path.clone()).or_default().unavailable = true;
        }
        if !offline.is_empty() {
            message.push_str(&format!(
                " · {} unavailable last-known matches · metadata list",
                offline.len()
            ));
        }
        tracing::debug!(target: "little_clip_search_timing",
            semantic = !vector.is_empty(), retrieval_ms, verification_ms, evidence_ms,
            total_ms = started.elapsed().as_secs_f64() * 1000.0,
            candidates = hits.len(), entries = entries.len(), "search ranking phases");
        Ok(super::super::Results {
            entries,
            message,
            sources,
            explanations,
        })
    }
}

pub(super) fn verify_candidates(
    ranked: Vec<Entry>,
    roots: Vec<SourceRoot>,
    stop: &AtomicBool,
) -> Result<(Vec<Entry>, String), String> {
    let verified = reader::verification::current(&ranked, &roots, stop)?;
    let stale = ranked.len() - verified.len();
    Ok((
        ranked
            .into_iter()
            .filter(|entry| verified.contains(&entry.path))
            .collect(),
        format!(
            "Semantic candidates (up to 500) · hybrid filename/keyword/meaning · indexed coverage only · {stale} stale/unavailable omitted"
        ),
    ))
}

// Preserve compatible audio/280-token image cache keys, distinguish frame inputs.
pub(super) fn media_hash(bytes: &[u8], modality: &str, budget: u32) -> Vec<u8> {
    let mut hash = Sha256::new();
    if modality == "image" && budget != 280 {
        hash.update(format!("vision-tokens:{budget}\0").as_bytes());
    }
    hash.update(bytes);
    hash.finalize().to_vec()
}
