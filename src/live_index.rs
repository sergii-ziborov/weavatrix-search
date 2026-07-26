use crate::error::{Error, Result};
use crate::index::{IndexOptions, IndexStatus, IndexUpdateReport, PersistentIndex};
use crate::options::SearchOptions;
use crate::query::SearchQuery;
use crate::report::{SearchBackend, SearchReport};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use weavatrix_scan::{ScanOptions, WatchEvent, WatchPlan, WatcherEventAdapter};

/// Runtime policy for a resident watcher-maintained index.
#[derive(Debug, Clone, Copy)]
pub struct LiveIndexOptions {
    /// Coalescing delay after the first filesystem event.
    pub debounce: Duration,
    /// Maximum queued callback messages before forcing a conservative rescan.
    pub event_buffer: usize,
    /// Rebuild existing indexes at startup to close watcher downtime gaps.
    pub rebuild_on_start: bool,
    /// Atomically save the index after every successful event batch.
    pub persist_each_batch: bool,
}

impl Default for LiveIndexOptions {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(75),
            event_buffer: 4_096,
            rebuild_on_start: true,
            persist_each_batch: false,
        }
    }
}

impl LiveIndexOptions {
    /// Sets the event coalescing delay.
    #[must_use]
    pub const fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Sets the bounded watcher callback queue.
    #[must_use]
    pub const fn with_event_buffer(mut self, event_buffer: usize) -> Self {
        self.event_buffer = event_buffer;
        self
    }

    /// Selects whether startup performs a complete freshness rebuild.
    #[must_use]
    pub const fn with_rebuild_on_start(mut self, enabled: bool) -> Self {
        self.rebuild_on_start = enabled;
        self
    }

    /// Selects whether every successful batch is atomically persisted.
    #[must_use]
    pub const fn with_persist_each_batch(mut self, enabled: bool) -> Self {
        self.persist_each_batch = enabled;
        self
    }

    /// Allows a trusted existing snapshot to open without a startup rebuild.
    #[must_use]
    pub const fn trust_existing_snapshot(mut self) -> Self {
        self.rebuild_on_start = false;
        self
    }
}

/// Ergonomic builder for a resident watcher-maintained index.
#[derive(Debug)]
pub struct LiveIndexBuilder {
    index_path: PathBuf,
    roots: Vec<PathBuf>,
    scan_options: ScanOptions,
    index_options: IndexOptions,
    live_options: LiveIndexOptions,
}

impl LiveIndexBuilder {
    /// Creates a builder with one root and safe freshness defaults.
    #[must_use]
    pub fn new(index_path: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        Self {
            index_path: index_path.into(),
            roots: vec![root.into()],
            scan_options: ScanOptions::default()
                .metadata_only()
                .selected_files_only()
                .with_content_validation(weavatrix_scan::ContentValidationPolicy::Fast),
            index_options: IndexOptions::default(),
            live_options: LiveIndexOptions::default(),
        }
    }

    /// Adds another watched root while preserving stable root identity.
    #[must_use]
    pub fn add_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.roots.push(root.into());
        self
    }

    /// Replaces repository discovery and content-safety policy.
    #[must_use]
    pub fn scan_options(mut self, options: ScanOptions) -> Self {
        self.scan_options = options;
        self
    }

    /// Replaces persistent-index resource and parallelism policy.
    #[must_use]
    pub fn index_options(mut self, options: IndexOptions) -> Self {
        self.index_options = options;
        self
    }

    /// Replaces watcher freshness, debounce, queue, and persistence policy.
    #[must_use]
    pub fn live_options(mut self, options: LiveIndexOptions) -> Self {
        self.live_options = options;
        self
    }

    /// Builds or opens the snapshot and starts native watchers.
    ///
    /// # Errors
    ///
    /// Returns index, root, watcher, or thread failures.
    pub fn start(self) -> Result<LiveIndex> {
        LiveIndex::start(
            self.index_path,
            self.roots,
            self.scan_options,
            self.index_options,
            self.live_options,
        )
    }
}

/// Health and update evidence for a resident index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveIndexStatus {
    /// Current immutable query snapshot.
    pub index: IndexStatus,
    /// Successful event batches.
    pub batches: u64,
    /// Files added, replaced, or removed across successful batches.
    pub changed_files: u64,
    /// Conservative full rebuilds caused by structural or overflow evidence.
    pub full_rebuilds: u64,
    /// Last background watcher or persistence failure.
    pub last_error: Option<String>,
    /// Whether RAM contains successful updates not yet persisted.
    pub dirty: bool,
    /// Whether the background update worker is still running.
    pub running: bool,
    /// Monotonic generation incremented after every successful update.
    pub generation: u64,
}

struct Shared {
    index: RwLock<PersistentIndex>,
    batches: AtomicU64,
    changed_files: AtomicU64,
    full_rebuilds: AtomicU64,
    last_error: Mutex<Option<String>>,
    running: AtomicBool,
    overflowed: Arc<AtomicBool>,
    dirty: AtomicBool,
    generation: Mutex<u64>,
    changed: Condvar,
}

enum LiveMessage {
    Event(notify::Result<Event>),
    Stop,
}

struct WatcherRuntime {
    sender: SyncSender<LiveMessage>,
    receiver: Receiver<LiveMessage>,
    watcher: RecommendedWatcher,
    overflowed: Arc<AtomicBool>,
    adapters: Vec<WatcherEventAdapter>,
}

/// Resident persistent index kept current by native filesystem notifications.
pub struct LiveIndex {
    shared: Arc<Shared>,
    sender: SyncSender<LiveMessage>,
    watcher: Option<RecommendedWatcher>,
    worker: Option<JoinHandle<()>>,
    index_path: PathBuf,
    scan_options: ScanOptions,
    persist_each_batch: bool,
}

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

    fn stop_inner(&mut self) -> Result<()> {
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

fn prepare_watcher(
    roots: &[PathBuf],
    scan_options: &ScanOptions,
    index_path: &Path,
    event_buffer: usize,
) -> Result<WatcherRuntime> {
    if event_buffer == 0 {
        return Err(Error::index(
            index_path,
            "live event buffer must be greater than zero",
        ));
    }
    let adapters = roots
        .iter()
        .map(|root| WatcherEventAdapter::with_options(root, scan_options))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (sender, receiver) = mpsc::sync_channel(event_buffer);
    let callback_sender = sender.clone();
    let overflowed = Arc::new(AtomicBool::new(false));
    let callback_overflowed = Arc::clone(&overflowed);
    let mut watcher = notify::recommended_watcher(move |event| {
        match callback_sender.try_send(LiveMessage::Event(event)) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                callback_overflowed.store(true, Ordering::Release);
            }
        }
    })
    .map_err(|error| Error::index(index_path, format!("watcher creation failed: {error}")))?;
    for root in roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| {
                Error::index(
                    index_path,
                    format!("failed to watch {}: {error}", root.display()),
                )
            })?;
    }
    Ok(WatcherRuntime {
        sender,
        receiver,
        watcher,
        overflowed,
        adapters,
    })
}

fn validate_roots(index: &PersistentIndex, roots: &[PathBuf], index_path: &Path) -> Result<()> {
    if roots.is_empty() {
        return Err(Error::index(index_path, "at least one root is required"));
    }
    let resolved = roots
        .iter()
        .map(|root| {
            root.canonicalize()
                .map_err(|source| Error::io(root, source))
        })
        .collect::<Result<Vec<_>>>()?;
    if resolved != index.roots() {
        return Err(Error::index(
            index_path,
            "requested roots do not match the persistent index",
        ));
    }
    Ok(())
}

impl Drop for LiveIndex {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

fn live_worker(
    receiver: &Receiver<LiveMessage>,
    shared: &Shared,
    index_path: &Path,
    scan_options: &ScanOptions,
    options: &LiveIndexOptions,
    adapters: &[WatcherEventAdapter],
) {
    while let Ok(message) = receiver.recv() {
        let LiveMessage::Event(first) = message else {
            break;
        };
        let started = Instant::now();
        let mut events = Vec::new();
        let mut force_rescan = shared.overflowed.swap(false, Ordering::AcqRel);
        let mut stop_after_batch = false;
        collect_event(first, &mut events, &mut force_rescan, shared);
        while started.elapsed() < options.debounce {
            let remaining = options.debounce.saturating_sub(started.elapsed());
            match receiver.recv_timeout(remaining) {
                Ok(LiveMessage::Event(event)) => {
                    collect_event(event, &mut events, &mut force_rescan, shared);
                }
                Ok(LiveMessage::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    stop_after_batch = true;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
            }
        }
        force_rescan |= shared.overflowed.swap(false, Ordering::AcqRel);
        if let Err(error) = apply_batch(
            shared,
            index_path,
            scan_options,
            options,
            adapters,
            &events,
            force_rescan,
        ) {
            set_last_error(shared, error.to_string());
        }
        if stop_after_batch {
            break;
        }
    }
}

fn collect_event(
    event: notify::Result<Event>,
    events: &mut Vec<Event>,
    force_rescan: &mut bool,
    shared: &Shared,
) {
    match event {
        Ok(event) => events.push(event),
        Err(error) => {
            *force_rescan = true;
            set_last_error(
                shared,
                format!("watcher reported lost or invalid events: {error}"),
            );
        }
    }
}

fn apply_batch(
    shared: &Shared,
    index_path: &Path,
    scan_options: &ScanOptions,
    options: &LiveIndexOptions,
    adapters: &[WatcherEventAdapter],
    events: &[Event],
    force_rescan: bool,
) -> Result<()> {
    let events = events
        .iter()
        .filter(|event| {
            !event
                .paths
                .iter()
                .any(|path| is_index_artifact(path, index_path))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut index = shared
        .index
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut reports = Vec::new();
    if force_rescan {
        let plan = WatchPlan {
            full_rescan: true,
            ..WatchPlan::default()
        };
        reports.push(index.update(0, &plan, scan_options.clone())?);
    } else {
        for (root_index, adapter) in adapters.iter().enumerate() {
            let plan = adapter.plan_notify(events.iter().cloned());
            if plan.full_rescan {
                reports.push(index.update(root_index, &plan, scan_options.clone())?);
                break;
            }
            if plan.changed.is_empty() && plan.removed.is_empty() {
                continue;
            }
            reports.push(index.update(root_index, &plan, scan_options.clone())?);
        }
    }
    if reports.is_empty() {
        return Ok(());
    }
    for report in &reports {
        record_update(shared, report);
    }
    if options.persist_each_batch {
        index.save(index_path)?;
        shared.dirty.store(false, Ordering::Release);
    }
    Ok(())
}

fn is_index_artifact(path: &Path, index_path: &Path) -> bool {
    if paths_equal(path, index_path) {
        return true;
    }
    let (Some(parent), Some(index_parent), Some(name), Some(index_name)) = (
        path.parent(),
        index_path.parent(),
        path.file_name(),
        index_path.file_name(),
    ) else {
        return false;
    };
    if !paths_equal(parent, index_parent) {
        return false;
    }
    let name = name.to_string_lossy();
    let index_name = index_name.to_string_lossy();
    let Some(suffix) = name.strip_prefix(index_name.as_ref()) else {
        return false;
    };
    suffix == ".lock" || suffix.starts_with(".tmp.") || suffix.starts_with(".backup.")
}

#[cfg(windows)]
fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = std::path::absolute(left)
        .unwrap_or_else(|_| left.to_path_buf())
        .to_string_lossy()
        .replace(r"\\?\", "");
    let right = std::path::absolute(right)
        .unwrap_or_else(|_| right.to_path_buf())
        .to_string_lossy()
        .replace(r"\\?\", "");
    left.eq_ignore_ascii_case(&right)
}

#[cfg(not(windows))]
fn paths_equal(left: &Path, right: &Path) -> bool {
    std::path::absolute(left).unwrap_or_else(|_| left.to_path_buf())
        == std::path::absolute(right).unwrap_or_else(|_| right.to_path_buf())
}

fn record_update(shared: &Shared, report: &IndexUpdateReport) {
    shared.dirty.store(true, Ordering::Release);
    shared.batches.fetch_add(1, Ordering::AcqRel);
    shared.changed_files.fetch_add(
        report
            .added
            .saturating_add(report.updated)
            .saturating_add(report.removed),
        Ordering::AcqRel,
    );
    if report.full_rebuild {
        shared.full_rebuilds.fetch_add(1, Ordering::AcqRel);
    }
    let mut generation = shared
        .generation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *generation = generation.saturating_add(1);
    shared.changed.notify_all();
}

fn set_last_error(shared: &Shared, error: String) {
    *shared
        .last_error
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
}
