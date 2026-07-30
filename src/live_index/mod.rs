mod api;
mod runtime;

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
