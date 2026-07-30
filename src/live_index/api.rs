use super::runtime::{live_worker, prepare_watcher, record_update, set_last_error, validate_roots};
use super::{
    Arc, AtomicBool, AtomicU64, Condvar, Duration, Error, IndexOptions, IndexUpdateReport,
    LiveIndex, LiveIndexBuilder, LiveIndexOptions, LiveIndexStatus, LiveMessage, Mutex, Ordering,
    PathBuf, PersistentIndex, Result, RwLock, ScanOptions, SearchBackend, SearchOptions,
    SearchQuery, SearchReport, Shared, WatchEvent, WatcherRuntime,
};

impl LiveIndex {
    /// Starts an ergonomic resident-index builder.
    #[must_use]
    pub fn builder(index_path: impl Into<PathBuf>, root: impl Into<PathBuf>) -> LiveIndexBuilder {
        LiveIndexBuilder::new(index_path, root)
    }

    /// Opens or builds an index, closes watcher downtime gaps according to
    /// `live_options`, and starts native recursive watchers for every root.
    ///
    /// # Errors
    ///
    /// Returns build, open, watcher, thread, or policy failures.
    pub fn start<I, P>(
        index_path: impl Into<PathBuf>,
        roots: I,
        scan_options: ScanOptions,
        index_options: IndexOptions,
        live_options: LiveIndexOptions,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let index_path = index_path.into();
        let roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        let watcher_roots = roots
            .iter()
            .map(|root| std::path::absolute(root).map_err(|source| Error::io(root, source)))
            .collect::<Result<Vec<_>>>()?;
        let runtime = prepare_watcher(
            &watcher_roots,
            &scan_options,
            &index_path,
            live_options.event_buffer,
        )?;
        let index = if index_path.exists() && !live_options.rebuild_on_start {
            let index = PersistentIndex::open(&index_path, index_options)?;
            validate_roots(&index, &roots, &index_path)?;
            index
        } else {
            PersistentIndex::build_and_save(
                &index_path,
                roots,
                scan_options.clone(),
                index_options,
            )?
            .0
        };
        Self::from_index_with_runtime(index_path, index, scan_options, live_options, runtime)
    }

    /// Starts watchers for an already validated in-memory index.
    ///
    /// The caller owns freshness before this method begins observing events.
    ///
    /// # Errors
    ///
    /// Returns watcher setup or invalid-policy failures.
    pub fn from_index(
        index_path: impl Into<PathBuf>,
        index: PersistentIndex,
        scan_options: ScanOptions,
        options: LiveIndexOptions,
    ) -> Result<Self> {
        let index_path = index_path.into();
        let roots = index.event_roots().to_vec();
        let runtime = prepare_watcher(&roots, &scan_options, &index_path, options.event_buffer)?;
        Self::from_index_with_runtime(index_path, index, scan_options, options, runtime)
    }

    fn from_index_with_runtime(
        index_path: PathBuf,
        index: PersistentIndex,
        scan_options: ScanOptions,
        options: LiveIndexOptions,
        runtime: WatcherRuntime,
    ) -> Result<Self> {
        let WatcherRuntime {
            sender,
            receiver,
            watcher,
            overflowed,
            adapters,
        } = runtime;
        let shared = Arc::new(Shared {
            index: RwLock::new(index),
            batches: AtomicU64::new(0),
            changed_files: AtomicU64::new(0),
            full_rebuilds: AtomicU64::new(0),
            last_error: Mutex::new(None),
            running: AtomicBool::new(true),
            overflowed,
            dirty: AtomicBool::new(false),
            generation: Mutex::new(0),
            changed: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker_path = index_path.clone();
        let worker_scan_options = scan_options.clone();
        let worker_options = options;
        let worker = std::thread::Builder::new()
            .name("weavatrix-search-live-index".to_owned())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    live_worker(
                        &receiver,
                        &worker_shared,
                        &worker_path,
                        &worker_scan_options,
                        &worker_options,
                        &adapters,
                    );
                }));
                if outcome.is_err() {
                    set_last_error(&worker_shared, "live index worker panicked".to_owned());
                }
                worker_shared.running.store(false, Ordering::Release);
                worker_shared.changed.notify_all();
            })
            .map_err(|source| Error::io(&index_path, source))?;
        Ok(Self {
            shared,
            sender,
            watcher: Some(watcher),
            worker: Some(worker),
            index_path,
            scan_options,
            persist_each_batch: options.persist_each_batch,
        })
    }

    /// Searches the current immutable snapshot while updates remain serialized
    /// behind a write lock.
    ///
    /// # Errors
    ///
    /// Returns query, decoding, archive, or resource-limit failures.
    pub fn search(&self, query: SearchQuery, options: SearchOptions) -> Result<SearchReport> {
        let index = self
            .shared
            .index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut report = index.search(query, options)?;
        report.backend = SearchBackend::LiveIndex;
        Ok(report)
    }

    /// Applies explicit normalized watcher events synchronously.
    ///
    /// This is useful for hosted watcher adapters and deterministic tests.
    ///
    /// # Errors
    ///
    /// Returns watcher-plan, scanner, index, or persistence failures.
    pub fn apply_events<I>(&self, root_index: usize, events: I) -> Result<IndexUpdateReport>
    where
        I: IntoIterator<Item = WatchEvent>,
    {
        let mut index = self
            .shared
            .index
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let report = index.update_events(root_index, events, self.scan_options.clone())?;
        record_update(&self.shared, &report);
        if self.persist_each_batch {
            index.save(&self.index_path)?;
            self.shared.dirty.store(false, Ordering::Release);
        }
        Ok(report)
    }

    /// Waits until the generation changes or the timeout expires.
    #[must_use]
    pub fn wait_for_update(&self, generation: u64, timeout: Duration) -> bool {
        let current = self
            .shared
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current != generation {
            return true;
        }
        let (current, _) = self
            .shared
            .changed
            .wait_timeout_while(current, timeout, |value| {
                *value == generation && self.shared.running.load(Ordering::Acquire)
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *current != generation
    }

    /// Returns a consistent health snapshot.
    #[must_use]
    pub fn status(&self) -> LiveIndexStatus {
        let index = self
            .shared
            .index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status();
        let last_error = self
            .shared
            .last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let generation = *self
            .shared
            .generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        LiveIndexStatus {
            index,
            batches: self.shared.batches.load(Ordering::Acquire),
            changed_files: self.shared.changed_files.load(Ordering::Acquire),
            full_rebuilds: self.shared.full_rebuilds.load(Ordering::Acquire),
            last_error,
            dirty: self.shared.dirty.load(Ordering::Acquire),
            running: self.shared.running.load(Ordering::Acquire),
            generation,
        }
    }

    /// Stops watchers, drains the current batch, persists a dirty snapshot,
    /// and joins the update worker.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker panicked or reported a background failure.
    pub fn stop(mut self) -> Result<()> {
        self.stop_inner()
    }

    pub(super) fn stop_inner(&mut self) -> Result<()> {
        self.watcher.take();
        let _ = self.sender.send(LiveMessage::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            return Err(Error::index(&self.index_path, "live index worker panicked"));
        }
        if self.shared.dirty.load(Ordering::Acquire) {
            self.shared
                .index
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .save(&self.index_path)?;
            self.shared.dirty.store(false, Ordering::Release);
        }
        if let Some(error) = self
            .shared
            .last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Err(Error::index(&self.index_path, error));
        }
        Ok(())
    }
}
