use crate::archive;
use crate::error::{Error, Result};
use crate::options::{EncodingMode, SearchOptions};
use crate::query::SearchQuery;
use crate::report::SearchReport;
use crate::searcher::{IndexedContent, search_indexed};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use weavatrix_scan::{
    ChangedContentVisitOutcome, ContentFileStatus, ContentVisitControl, ContentVisitEvent,
    ContentVisitReport, MultiContentVisitReport, MultiScanner, ScanOptions, Scanner, WatchEvent,
    WatchPlan, WatcherEventAdapter,
};

const MAGIC: &[u8; 8] = b"WVXIDX01";
const FORMAT_VERSION: u32 = 1;
const CHECKSUM_BYTES: u64 = 32;
// 512 bits keeps short/medium source files selective while saving 64 bytes
// per entry versus the original prototype (12.2 MiB at 200k files).
const BLOOM_WORDS: usize = 8;
const MAX_ROOTS: usize = 4_096;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "windows")]
const PLATFORM_ID: u8 = 1;
#[cfg(target_os = "linux")]
const PLATFORM_ID: u8 = 2;
#[cfg(target_os = "macos")]
const PLATFORM_ID: u8 = 3;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
const PLATFORM_ID: u8 = 4;

/// Resource and parallelism policy for persistent indexes.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Maximum selected files accepted by one index.
    pub max_entries: u64,
    /// Maximum raw source bytes retained by one index.
    pub max_content_bytes: u64,
    /// Maximum serialized index bytes accepted on load or save.
    pub max_index_bytes: u64,
    /// Maximum encoded bytes accepted for one root or relative path.
    pub max_path_bytes: usize,
    /// Content workers used while building or updating the index.
    pub build_parallelism: usize,
    /// Workers used to verify candidate content during a query.
    pub search_parallelism: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        let parallelism = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .clamp(1, 32);
        Self {
            max_entries: 2_000_000,
            max_content_bytes: 64 * 1024 * 1024 * 1024,
            max_index_bytes: 96 * 1024 * 1024 * 1024,
            max_path_bytes: 1024 * 1024,
            build_parallelism: parallelism,
            search_parallelism: parallelism,
        }
    }
}

impl IndexOptions {
    /// Sets build and query worker counts.
    #[must_use]
    pub fn with_parallelism(mut self, workers: usize) -> Self {
        self.build_parallelism = workers;
        self.search_parallelism = workers;
        self
    }

    /// Sets content workers used while building or updating.
    #[must_use]
    pub const fn with_build_parallelism(mut self, workers: usize) -> Self {
        self.build_parallelism = workers;
        self
    }

    /// Sets candidate-verification workers used by queries.
    #[must_use]
    pub const fn with_search_parallelism(mut self, workers: usize) -> Self {
        self.search_parallelism = workers;
        self
    }

    /// Sets the maximum selected-file count.
    #[must_use]
    pub const fn with_max_entries(mut self, max_entries: u64) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Sets the maximum retained source bytes.
    #[must_use]
    pub const fn with_max_content_bytes(mut self, max_content_bytes: u64) -> Self {
        self.max_content_bytes = max_content_bytes;
        self
    }

    /// Sets the maximum serialized index bytes.
    #[must_use]
    pub const fn with_max_index_bytes(mut self, max_index_bytes: u64) -> Self {
        self.max_index_bytes = max_index_bytes;
        self
    }

    /// Sets the maximum encoded length of one root or relative path.
    #[must_use]
    pub const fn with_max_path_bytes(mut self, max_path_bytes: usize) -> Self {
        self.max_path_bytes = max_path_bytes;
        self
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.build_parallelism == 0 || self.search_parallelism == 0 {
            return Err(Error::index(path, "parallelism must be greater than zero"));
        }
        if self.max_entries == 0 {
            return Err(Error::index(path, "max_entries must be greater than zero"));
        }
        if self.max_content_bytes == 0 || self.max_index_bytes == 0 {
            return Err(Error::index(
                path,
                "content and index byte limits must be greater than zero",
            ));
        }
        if self.max_path_bytes == 0 {
            return Err(Error::index(
                path,
                "max_path_bytes must be greater than zero",
            ));
        }
        Ok(())
    }
}

/// Stable metadata returned after a full index build.
#[derive(Debug)]
pub struct IndexBuildReport {
    /// Indexed roots in insertion order.
    pub roots: Vec<PathBuf>,
    /// Selected files retained by the index.
    pub files: u64,
    /// Raw source bytes retained by the index.
    pub content_bytes: u64,
    /// Deterministic root/path/content revision.
    pub revision: String,
    /// Scanner evidence for the full build.
    pub scan: MultiContentVisitReport,
}

/// Stable metadata returned after applying one watcher plan.
#[derive(Debug)]
pub struct IndexUpdateReport {
    /// Newly selected paths.
    pub added: u64,
    /// Existing paths whose bytes were replaced.
    pub updated: u64,
    /// Existing paths removed or no longer selected.
    pub removed: u64,
    /// Selected files retained after the update.
    pub files: u64,
    /// Raw source bytes retained after the update.
    pub content_bytes: u64,
    /// New deterministic index revision.
    pub revision: String,
    /// Whether conservative watcher evidence required a complete rebuild.
    pub full_rebuild: bool,
    /// Changed-file scanner evidence when no rebuild was required.
    pub changed_scan: Option<ContentVisitReport>,
}

/// Cheap status suitable for health endpoints and live-index diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    /// Indexed roots in insertion order.
    pub roots: Vec<PathBuf>,
    /// Selected files retained by the index.
    pub files: u64,
    /// Raw source bytes retained by the index.
    pub content_bytes: u64,
    /// Deterministic root/path/content revision.
    pub revision: String,
}

#[derive(Debug)]
struct IndexEntry {
    root_index: usize,
    path: String,
    content: Vec<u8>,
    content_hash: [u8; 32],
    prefilterable: bool,
    bloom: [u64; BLOOM_WORDS],
}

impl IndexEntry {
    fn key(&self) -> (usize, &str) {
        (self.root_index, &self.path)
    }

    fn from_parts(root_index: usize, path: String, content: Vec<u8>) -> Self {
        let content_hash: [u8; 32] = Sha256::digest(&content).into();
        let prefilterable = std::str::from_utf8(&content).is_ok() && archive::kind(&path).is_none();
        let bloom = if prefilterable {
            trigram_bloom(&content)
        } else {
            [0; BLOOM_WORDS]
        };
        Self {
            root_index,
            path,
            content,
            content_hash,
            prefilterable,
            bloom,
        }
    }

    fn may_match(&self, alternatives: &[Vec<u32>]) -> bool {
        !self.prefilterable
            || alternatives.iter().any(|trigrams| {
                trigrams
                    .iter()
                    .all(|trigram| bloom_contains(&self.bloom, *trigram))
            })
    }
}

struct EntryBuilder {
    root_index: usize,
    path: String,
    expected_bytes: u64,
    content: Vec<u8>,
}

impl EntryBuilder {
    fn new(root_index: usize, path: &str, expected_bytes: u64) -> Result<Self> {
        let capacity = usize::try_from(expected_bytes).map_err(|_| {
            Error::index(
                path,
                format!("{expected_bytes} source bytes do not fit in memory"),
            )
        })?;
        let mut content = Vec::new();
        content
            .try_reserve_exact(capacity)
            .map_err(|error| Error::index(path, format!("content allocation failed: {error}")))?;
        Ok(Self {
            root_index,
            path: path.to_owned(),
            expected_bytes,
            content,
        })
    }

    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        self.content.try_reserve(bytes.len()).map_err(|error| {
            Error::index(&self.path, format!("content allocation failed: {error}"))
        })?;
        self.content.extend_from_slice(bytes);
        Ok(())
    }

    fn finish(self, status: ContentFileStatus, bytes_read: u64) -> Option<IndexEntry> {
        if status == ContentFileStatus::Changed
            || bytes_read != self.expected_bytes
            || bytes_read != u64::try_from(self.content.len()).unwrap_or(u64::MAX)
        {
            return None;
        }
        Some(IndexEntry::from_parts(
            self.root_index,
            self.path,
            self.content,
        ))
    }
}

/// Ergonomic builder for one persistent multi-root snapshot.
#[derive(Debug)]
pub struct IndexBuilder {
    roots: Vec<PathBuf>,
    scan_options: ScanOptions,
    index_options: IndexOptions,
}

impl IndexBuilder {
    /// Creates a builder with one repository root and safe defaults.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            roots: vec![root.into()],
            scan_options: ScanOptions::default()
                .metadata_only()
                .selected_files_only()
                .with_content_validation(weavatrix_scan::ContentValidationPolicy::Fast),
            index_options: IndexOptions::default(),
        }
    }

    /// Adds another root while preserving stable root order.
    #[must_use]
    pub fn add_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.roots.push(root.into());
        self
    }

    /// Adds several roots while preserving insertion order.
    #[must_use]
    pub fn extend_roots<I, P>(mut self, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.roots.extend(roots.into_iter().map(Into::into));
        self
    }

    /// Replaces repository discovery, ignore, and content-safety policy.
    #[must_use]
    pub fn scan_options(mut self, options: ScanOptions) -> Self {
        self.scan_options = options;
        self
    }

    /// Replaces index resource and parallelism policy.
    #[must_use]
    pub fn index_options(mut self, options: IndexOptions) -> Self {
        self.index_options = options;
        self
    }

    /// Builds an in-memory snapshot.
    ///
    /// # Errors
    ///
    /// Returns scanner, allocation, or resource-limit failures.
    pub fn build(self) -> Result<(PersistentIndex, IndexBuildReport)> {
        PersistentIndex::build(self.roots, self.scan_options, self.index_options)
    }

    /// Builds and atomically saves a snapshot.
    ///
    /// # Errors
    ///
    /// Returns build or persistence failures.
    pub fn build_and_save(
        self,
        path: impl AsRef<Path>,
    ) -> Result<(PersistentIndex, IndexBuildReport)> {
        PersistentIndex::build_and_save(path, self.roots, self.scan_options, self.index_options)
    }
}

/// Immutable query snapshot with explicit mutation methods for watcher deltas.
#[derive(Debug)]
pub struct PersistentIndex {
    roots: Vec<PathBuf>,
    event_roots: Vec<PathBuf>,
    entries: Vec<IndexEntry>,
    revision: String,
    content_bytes: u64,
    options: IndexOptions,
    storage_path: Mutex<Option<PathBuf>>,
}

impl PersistentIndex {
    /// Starts an ergonomic persistent-index builder.
    #[must_use]
    pub fn builder(root: impl Into<PathBuf>) -> IndexBuilder {
        IndexBuilder::new(root)
    }

    /// Builds a complete in-memory index through Weavatrix Scan.
    ///
    /// # Errors
    ///
    /// Returns scanner, allocation, limit, or invalid-policy failures.
    #[allow(clippy::too_many_lines)]
    pub fn build<I, P>(
        roots: I,
        scan_options: ScanOptions,
        options: IndexOptions,
    ) -> Result<(Self, IndexBuildReport)>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        options.validate(Path::new("<memory>"))?;
        let roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        if roots.is_empty() {
            return Err(Error::index("<memory>", "at least one root is required"));
        }
        if roots.len() > MAX_ROOTS {
            return Err(Error::index(
                "<memory>",
                format!("root count {} exceeds {MAX_ROOTS}", roots.len()),
            ));
        }
        let event_roots = roots
            .iter()
            .map(|root| std::path::absolute(root).map_err(|source| Error::io(root, source)))
            .collect::<Result<Vec<_>>>()?;
        let entries = Arc::new(Mutex::new(Vec::new()));
        let failure = Arc::new(Mutex::new(None));
        let retained_bytes = Arc::new(AtomicU64::new(0));
        let retained_entries = Arc::new(AtomicU64::new(0));
        let worker_entries = Arc::clone(&entries);
        let worker_failure = Arc::clone(&failure);
        let worker_bytes = Arc::clone(&retained_bytes);
        let worker_count = Arc::clone(&retained_entries);
        let limits = options.clone();
        let mut roots_iter = roots.iter().cloned();
        let first = roots_iter
            .next()
            .ok_or_else(|| Error::index("<memory>", "at least one root is required"))?;
        let scanner = roots_iter.fold(
            MultiScanner::new(first)
                .options(scan_options.with_content_parallelism(options.build_parallelism)),
            MultiScanner::add_root,
        );
        let scan = scanner.visit_content_streaming(move |_, _| {
            let entries = Arc::clone(&worker_entries);
            let failure = Arc::clone(&worker_failure);
            let retained_bytes = Arc::clone(&worker_bytes);
            let retained_entries = Arc::clone(&worker_count);
            let limits = limits.clone();
            let mut file = None;
            move |event| {
                if has_failure(&failure) {
                    return ContentVisitControl::Quit;
                }
                match event {
                    ContentVisitEvent::FileStart { file: opened, .. } => {
                        match EntryBuilder::new(opened.root_index, opened.relative, opened.bytes) {
                            Ok(builder) => file = Some(builder),
                            Err(error) => {
                                set_failure(&failure, error);
                                return ContentVisitControl::Quit;
                            }
                        }
                    }
                    ContentVisitEvent::Chunk { bytes, .. } => {
                        if let Some(builder) = &mut file {
                            let chunk = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                            let previous = retained_bytes.fetch_add(chunk, Ordering::AcqRel);
                            if previous.saturating_add(chunk) > limits.max_content_bytes {
                                set_failure(
                                    &failure,
                                    Error::index(
                                        &builder.path,
                                        format!(
                                            "content exceeds {} bytes",
                                            limits.max_content_bytes
                                        ),
                                    ),
                                );
                                return ContentVisitControl::Quit;
                            }
                            if let Err(error) = builder.push(bytes) {
                                set_failure(&failure, error);
                                return ContentVisitControl::Quit;
                            }
                        }
                    }
                    ContentVisitEvent::FileEnd {
                        status, bytes_read, ..
                    } => {
                        if let Some(entry) = file
                            .take()
                            .and_then(|builder| builder.finish(status, bytes_read))
                        {
                            let count = retained_entries.fetch_add(1, Ordering::AcqRel) + 1;
                            if count > limits.max_entries {
                                set_failure(
                                    &failure,
                                    Error::index(
                                        &entry.path,
                                        format!("entry count exceeds {}", limits.max_entries),
                                    ),
                                );
                                return ContentVisitControl::Quit;
                            }
                            entries
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .push(entry);
                        }
                    }
                }
                ContentVisitControl::Continue
            }
        })?;
        if let Some(error) = take_failure(&failure) {
            return Err(error);
        }
        let mut entries = std::mem::take(
            &mut *entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        entries.sort_unstable_by(|left, right| left.key().cmp(&right.key()));
        validate_unique_entries(&entries, Path::new("<memory>"))?;
        let indexed_roots = scan
            .reports
            .iter()
            .map(|report| report.root.clone())
            .collect::<Vec<_>>();
        let content_bytes = content_bytes(&entries, Path::new("<memory>"))?;
        let revision = revision(&indexed_roots, &entries);
        let files = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let report = IndexBuildReport {
            roots: indexed_roots.clone(),
            files,
            content_bytes,
            revision: revision.clone(),
            scan,
        };
        Ok((
            Self {
                roots: indexed_roots,
                event_roots,
                entries,
                revision,
                content_bytes,
                options,
                storage_path: Mutex::new(None),
            },
            report,
        ))
    }

    /// Builds and atomically saves an index.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::build`] plus persistence failures.
    pub fn build_and_save<I, P>(
        path: impl AsRef<Path>,
        roots: I,
        scan_options: ScanOptions,
        options: IndexOptions,
    ) -> Result<(Self, IndexBuildReport)>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        let scan_options =
            scan_options_with_storage_exclusion(&roots, scan_options, path.as_ref())?;
        let (index, report) = Self::build(roots, scan_options, options)?;
        index.save(path)?;
        Ok((index, report))
    }

    /// Opens and fully validates a persistent index.
    ///
    /// # Errors
    ///
    /// Returns I/O, format, platform, checksum, allocation, or limit failures.
    #[allow(clippy::too_many_lines)]
    pub fn open(path: impl AsRef<Path>, options: IndexOptions) -> Result<Self> {
        let path = path.as_ref();
        let storage_path = std::path::absolute(path).map_err(|source| Error::io(path, source))?;
        options.validate(path)?;
        let metadata = fs::metadata(path).map_err(|source| Error::io(path, source))?;
        if metadata.len() > options.max_index_bytes {
            return Err(Error::index(
                path,
                format!(
                    "file size {} exceeds {} bytes",
                    metadata.len(),
                    options.max_index_bytes
                ),
            ));
        }
        let file = File::open(path).map_err(|source| Error::io(path, source))?;
        let mut reader = IndexReader::new(file, options.max_index_bytes);
        if reader.bytes(8)? != MAGIC {
            return Err(Error::index(path, "invalid magic"));
        }
        let version = reader.u32()?;
        if version != FORMAT_VERSION {
            return Err(Error::index(
                path,
                format!("format version {version} is not supported"),
            ));
        }
        let platform = reader.u8()?;
        if platform != PLATFORM_ID {
            return Err(Error::index(
                path,
                format!("index platform {platform} does not match runtime {PLATFORM_ID}"),
            ));
        }
        let _flags = reader.u8()?;
        let _reserved = reader.u16()?;
        let root_count = usize::try_from(reader.u32()?)
            .map_err(|_| Error::index(path, "root count does not fit usize"))?;
        if root_count == 0 || root_count > MAX_ROOTS {
            return Err(Error::index(
                path,
                format!("invalid root count {root_count}"),
            ));
        }
        let entry_count = reader.u64()?;
        if entry_count > options.max_entries {
            return Err(Error::index(
                path,
                format!("entry count {entry_count} exceeds {}", options.max_entries),
            ));
        }
        let declared_content_bytes = reader.u64()?;
        if declared_content_bytes > options.max_content_bytes {
            return Err(Error::index(
                path,
                format!(
                    "content bytes {declared_content_bytes} exceed {}",
                    options.max_content_bytes
                ),
            ));
        }
        let declared_revision = reader.array::<32>()?;
        let mut roots = Vec::new();
        roots
            .try_reserve_exact(root_count)
            .map_err(|error| Error::index(path, format!("root allocation failed: {error}")))?;
        for _ in 0..root_count {
            let encoded = reader.length_prefixed(options.max_path_bytes)?;
            roots.push(decode_path(&encoded).map_err(|message| Error::index(path, message))?);
        }
        let mut event_roots = Vec::new();
        event_roots.try_reserve_exact(root_count).map_err(|error| {
            Error::index(path, format!("event-root allocation failed: {error}"))
        })?;
        for _ in 0..root_count {
            let encoded = reader.length_prefixed(options.max_path_bytes)?;
            event_roots.push(decode_path(&encoded).map_err(|message| Error::index(path, message))?);
        }
        let entry_capacity = usize::try_from(entry_count)
            .map_err(|_| Error::index(path, "entry count does not fit usize"))?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(entry_capacity)
            .map_err(|error| Error::index(path, format!("entry allocation failed: {error}")))?;
        let mut observed_content_bytes = 0_u64;
        for _ in 0..entry_count {
            let root_index = usize::try_from(reader.u32()?)
                .map_err(|_| Error::index(path, "root index does not fit usize"))?;
            if root_index >= roots.len() {
                return Err(Error::index(path, "entry root index is out of range"));
            }
            let encoded_path = reader.length_prefixed(options.max_path_bytes)?;
            let relative = String::from_utf8(encoded_path)
                .map_err(|_| Error::index(path, "relative path is not UTF-8"))?;
            let content_len = reader.u64()?;
            observed_content_bytes = observed_content_bytes
                .checked_add(content_len)
                .ok_or_else(|| Error::index(path, "content byte count overflow"))?;
            if observed_content_bytes > options.max_content_bytes {
                return Err(Error::index(
                    path,
                    format!("content exceeds {} bytes", options.max_content_bytes),
                ));
            }
            let content_hash = reader.array::<32>()?;
            let prefilterable = match reader.u8()? {
                0 => false,
                1 => true,
                value => {
                    return Err(Error::index(
                        path,
                        format!("invalid prefilter flag {value}"),
                    ));
                }
            };
            let mut bloom = [0_u64; BLOOM_WORDS];
            for word in &mut bloom {
                *word = reader.u64()?;
            }
            let content_size = usize::try_from(content_len)
                .map_err(|_| Error::index(path, "file content does not fit usize"))?;
            let content = reader.bytes(content_size)?;
            entries.push(IndexEntry {
                root_index,
                path: relative,
                content,
                content_hash,
                prefilterable,
                bloom,
            });
        }
        let checksum = reader.finish()?;
        if observed_content_bytes != declared_content_bytes {
            return Err(Error::index(
                path,
                "declared content byte count does not match entries",
            ));
        }
        validate_unique_entries(&entries, path)?;
        let computed_revision = revision_bytes(&roots, &entries);
        if computed_revision != declared_revision {
            return Err(Error::index(
                path,
                "revision evidence does not match entries",
            ));
        }
        if !checksum.valid {
            return Err(Error::index(path, "checksum mismatch"));
        }
        Ok(Self {
            roots,
            event_roots,
            entries,
            revision: hex(&computed_revision),
            content_bytes: observed_content_bytes,
            options,
            storage_path: Mutex::new(Some(storage_path)),
        })
    }

    /// Atomically saves this index with a whole-file SHA-256 checksum.
    ///
    /// # Errors
    ///
    /// Returns path, lock, size, serialization, or I/O failures.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        self.options.validate(path)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        let _lock = IndexLock::acquire(path)?;
        let temp = auxiliary_path(path, "tmp");
        let result = self
            .write_file(&temp)
            .and_then(|()| replace_file(&temp, path));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        let storage_path = std::path::absolute(path).map_err(|source| Error::io(path, source))?;
        *self
            .storage_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(storage_path);
        Ok(())
    }

    fn write_file(&self, path: &Path) -> Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| Error::io(path, source))?;
        let mut writer = IndexWriter::new(file, self.options.max_index_bytes);
        writer.bytes(MAGIC)?;
        writer.u32(FORMAT_VERSION)?;
        writer.u8(PLATFORM_ID)?;
        writer.u8(0)?;
        writer.u16(0)?;
        writer.u32(
            u32::try_from(self.roots.len())
                .map_err(|_| Error::index(path, "root count exceeds u32"))?,
        )?;
        writer.u64(
            u64::try_from(self.entries.len())
                .map_err(|_| Error::index(path, "entry count exceeds u64"))?,
        )?;
        writer.u64(self.content_bytes)?;
        writer.bytes(&revision_bytes(&self.roots, &self.entries))?;
        for root in &self.roots {
            writer.length_prefixed(&encode_path(root), self.options.max_path_bytes)?;
        }
        for root in &self.event_roots {
            writer.length_prefixed(&encode_path(root), self.options.max_path_bytes)?;
        }
        for entry in &self.entries {
            writer.u32(
                u32::try_from(entry.root_index)
                    .map_err(|_| Error::index(path, "root index exceeds u32"))?,
            )?;
            writer.length_prefixed(entry.path.as_bytes(), self.options.max_path_bytes)?;
            writer.u64(
                u64::try_from(entry.content.len())
                    .map_err(|_| Error::index(path, "file content exceeds u64"))?,
            )?;
            writer.bytes(&entry.content_hash)?;
            writer.u8(u8::from(entry.prefilterable))?;
            for word in entry.bloom {
                writer.u64(word)?;
            }
            writer.bytes(&entry.content)?;
        }
        writer.finish(path)
    }

    /// Searches the indexed snapshot with bounded parallel verification.
    ///
    /// # Errors
    ///
    /// Returns query, decoding, archive, or resource-limit failures.
    pub fn search(&self, query: SearchQuery, options: SearchOptions) -> Result<SearchReport> {
        self.search_with_parallelism(query, options, self.options.search_parallelism)
    }

    /// Searches with an explicit worker count.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::search`] and rejects zero workers.
    // Owning the query keeps temporary literals ergonomic and mirrors Searcher.
    #[allow(clippy::needless_pass_by_value)]
    pub fn search_with_parallelism(
        &self,
        query: SearchQuery,
        options: SearchOptions,
        parallelism: usize,
    ) -> Result<SearchReport> {
        if parallelism == 0 {
            return Err(Error::index(
                "<memory>",
                "search parallelism must be greater than zero",
            ));
        }
        let alternatives = match options.encoding {
            EncodingMode::Auto | EncodingMode::Utf8 => query.prefilter_trigrams(options.case),
            EncodingMode::Utf16Le | EncodingMode::Utf16Be | EncodingMode::Label(_) => None,
        };
        let files = self
            .entries
            .iter()
            .filter(|entry| {
                alternatives
                    .as_deref()
                    .is_none_or(|trigrams| entry.may_match(trigrams))
            })
            .map(|entry| IndexedContent {
                root_index: entry.root_index,
                path: &entry.path,
                bytes: &entry.content,
            })
            .collect::<Vec<_>>();
        let candidate_files = u64::try_from(files.len()).unwrap_or(u64::MAX);
        let indexed_files = u64::try_from(self.entries.len()).unwrap_or(u64::MAX);
        search_indexed(
            self.roots.clone(),
            &files,
            &query,
            options,
            parallelism,
            self.revision.clone(),
            indexed_files,
            candidate_files,
            alternatives.is_some(),
        )
    }

    /// Applies a safe changed-file watcher plan without directory traversal.
    ///
    /// Plans that can affect selection rebuild the complete multi-root index.
    ///
    /// # Errors
    ///
    /// Returns invalid-root, scanner, allocation, or limit failures.
    #[allow(clippy::too_many_lines)]
    pub fn update(
        &mut self,
        root_index: usize,
        plan: &WatchPlan,
        scan_options: ScanOptions,
    ) -> Result<IndexUpdateReport> {
        let root = self.roots.get(root_index).cloned().ok_or_else(|| {
            Error::index("<memory>", format!("root index {root_index} is invalid"))
        })?;
        if plan.full_rescan {
            return self.rebuild(scan_options);
        }
        let entries = Arc::new(Mutex::new(Vec::new()));
        let failure = Arc::new(Mutex::new(None));
        let retained_bytes = Arc::new(AtomicU64::new(0));
        let worker_entries = Arc::clone(&entries);
        let worker_failure = Arc::clone(&failure);
        let worker_bytes = Arc::clone(&retained_bytes);
        let limits = self.options.clone();
        let outcome = Scanner::new(root)
            .options(
                scan_options
                    .clone()
                    .with_content_parallelism(self.options.build_parallelism),
            )
            .visit_changed_content_streaming(plan, move |_| {
                let entries = Arc::clone(&worker_entries);
                let failure = Arc::clone(&worker_failure);
                let retained_bytes = Arc::clone(&worker_bytes);
                let limits = limits.clone();
                let mut file = None;
                move |event| {
                    if has_failure(&failure) {
                        return ContentVisitControl::Quit;
                    }
                    match event {
                        ContentVisitEvent::FileStart { file: opened, .. } => {
                            match EntryBuilder::new(root_index, opened.relative, opened.bytes) {
                                Ok(builder) => file = Some(builder),
                                Err(error) => {
                                    set_failure(&failure, error);
                                    return ContentVisitControl::Quit;
                                }
                            }
                        }
                        ContentVisitEvent::Chunk { bytes, .. } => {
                            if let Some(builder) = &mut file {
                                let chunk = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                                let previous = retained_bytes.fetch_add(chunk, Ordering::AcqRel);
                                if previous.saturating_add(chunk) > limits.max_content_bytes {
                                    set_failure(
                                        &failure,
                                        Error::index(
                                            &builder.path,
                                            format!(
                                                "changed content exceeds {} bytes",
                                                limits.max_content_bytes
                                            ),
                                        ),
                                    );
                                    return ContentVisitControl::Quit;
                                }
                                if let Err(error) = builder.push(bytes) {
                                    set_failure(&failure, error);
                                    return ContentVisitControl::Quit;
                                }
                            }
                        }
                        ContentVisitEvent::FileEnd {
                            status, bytes_read, ..
                        } => {
                            if let Some(entry) = file
                                .take()
                                .and_then(|builder| builder.finish(status, bytes_read))
                            {
                                entries
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .push(entry);
                            }
                        }
                    }
                    ContentVisitControl::Continue
                }
            })?;
        if let Some(error) = take_failure(&failure) {
            return Err(error);
        }
        match outcome {
            ChangedContentVisitOutcome::FullRescanRequired => self.rebuild(scan_options),
            ChangedContentVisitOutcome::Visited(report) => {
                let mut changed_entries = std::mem::take(
                    &mut *entries
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                changed_entries.sort_unstable_by(|left, right| left.key().cmp(&right.key()));
                validate_unique_entries(&changed_entries, Path::new("<memory>"))?;
                let affected = plan
                    .changed
                    .iter()
                    .chain(&plan.removed)
                    .chain(&report.removed)
                    .cloned()
                    .chain(changed_entries.iter().map(|entry| entry.path.clone()))
                    .collect::<BTreeSet<_>>();
                let old_entries = affected
                    .iter()
                    .filter_map(|path| {
                        entry_position(&self.entries, root_index, path)
                            .ok()
                            .map(|position| {
                                let entry = &self.entries[position];
                                (path.clone(), (entry.content_hash, entry.content.len()))
                            })
                    })
                    .collect::<BTreeMap<_, _>>();
                let updated = changed_entries
                    .iter()
                    .filter(|entry| {
                        old_entries
                            .get(&entry.path)
                            .is_some_and(|(hash, _)| hash != &entry.content_hash)
                    })
                    .count();
                let retained = changed_entries
                    .iter()
                    .filter(|entry| old_entries.contains_key(&entry.path))
                    .count();
                let added = changed_entries.len().saturating_sub(retained);
                let removed = old_entries.len().saturating_sub(retained);
                let old_content_bytes =
                    old_entries.values().try_fold(0_u64, |total, (_, bytes)| {
                        total
                            .checked_add(u64::try_from(*bytes).map_err(|_| {
                                Error::index("<memory>", "existing content size exceeds u64")
                            })?)
                            .ok_or_else(|| Error::index("<memory>", "content byte count overflow"))
                    })?;
                let changed_content_bytes = content_bytes(&changed_entries, Path::new("<memory>"))?;
                let next_len = self
                    .entries
                    .len()
                    .checked_sub(old_entries.len())
                    .and_then(|count| count.checked_add(changed_entries.len()))
                    .ok_or_else(|| Error::index("<memory>", "entry count overflow"))?;
                if u64::try_from(next_len).unwrap_or(u64::MAX) > self.options.max_entries {
                    return Err(Error::index(
                        "<memory>",
                        format!("entry count exceeds {}", self.options.max_entries),
                    ));
                }
                let next_content_bytes = self
                    .content_bytes
                    .checked_sub(old_content_bytes)
                    .and_then(|bytes| bytes.checked_add(changed_content_bytes))
                    .ok_or_else(|| Error::index("<memory>", "content byte count overflow"))?;
                if next_content_bytes > self.options.max_content_bytes {
                    return Err(Error::index(
                        "<memory>",
                        format!("content exceeds {} bytes", self.options.max_content_bytes),
                    ));
                }
                self.entries
                    .try_reserve(changed_entries.len())
                    .map_err(|error| {
                        Error::index("<memory>", format!("entry allocation failed: {error}"))
                    })?;
                let changed_paths = changed_entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect::<BTreeSet<_>>();
                for entry in changed_entries {
                    match entry_position(&self.entries, root_index, &entry.path) {
                        Ok(position) => self.entries[position] = entry,
                        Err(position) => self.entries.insert(position, entry),
                    }
                }
                for path in affected.difference(&changed_paths) {
                    if let Ok(position) = entry_position(&self.entries, root_index, path) {
                        self.entries.remove(position);
                    }
                }
                debug_assert!(
                    validate_unique_entries(&self.entries, Path::new("<memory>")).is_ok()
                );
                self.content_bytes = next_content_bytes;
                self.revision = revision(&self.roots, &self.entries);
                Ok(IndexUpdateReport {
                    added: u64::try_from(added).unwrap_or(u64::MAX),
                    updated: u64::try_from(updated).unwrap_or(u64::MAX),
                    removed: u64::try_from(removed).unwrap_or(u64::MAX),
                    files: u64::try_from(self.entries.len()).unwrap_or(u64::MAX),
                    content_bytes: self.content_bytes,
                    revision: self.revision.clone(),
                    full_rebuild: false,
                    changed_scan: Some(report.content),
                })
            }
        }
    }

    /// Converts raw watcher events for one root and applies their deterministic
    /// plan.
    ///
    /// # Errors
    ///
    /// Returns watcher-adapter or update failures.
    pub fn update_events<I>(
        &mut self,
        root_index: usize,
        events: I,
        scan_options: ScanOptions,
    ) -> Result<IndexUpdateReport>
    where
        I: IntoIterator<Item = WatchEvent>,
    {
        let root = self.event_roots.get(root_index).ok_or_else(|| {
            Error::index("<memory>", format!("root index {root_index} is invalid"))
        })?;
        let adapter = WatcherEventAdapter::with_options(root, &scan_options)?;
        let plan = adapter.plan(events);
        self.update(root_index, &plan, scan_options)
    }

    fn rebuild(&mut self, scan_options: ScanOptions) -> Result<IndexUpdateReport> {
        let storage_path = self
            .storage_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let scan_options = self.with_storage_exclusion(scan_options, storage_path.as_deref())?;
        let old = self
            .entries
            .iter()
            .map(|entry| ((entry.root_index, entry.path.clone()), entry.content_hash))
            .collect::<BTreeMap<_, _>>();
        let (new, report) =
            Self::build(self.event_roots.clone(), scan_options, self.options.clone())?;
        *new.storage_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = storage_path;
        let current = new
            .entries
            .iter()
            .map(|entry| ((entry.root_index, entry.path.clone()), entry.content_hash))
            .collect::<BTreeMap<_, _>>();
        let added = current.keys().filter(|key| !old.contains_key(*key)).count();
        let removed = old.keys().filter(|key| !current.contains_key(*key)).count();
        let updated = current
            .iter()
            .filter(|(key, hash)| old.get(*key).is_some_and(|old_hash| old_hash != *hash))
            .count();
        *self = new;
        Ok(IndexUpdateReport {
            added: u64::try_from(added).unwrap_or(u64::MAX),
            updated: u64::try_from(updated).unwrap_or(u64::MAX),
            removed: u64::try_from(removed).unwrap_or(u64::MAX),
            files: u64::try_from(self.entries.len()).unwrap_or(u64::MAX),
            content_bytes: self.content_bytes,
            revision: self.revision.clone(),
            full_rebuild: true,
            changed_scan: report.scan.reports.into_iter().next(),
        })
    }

    /// Returns immutable index health evidence.
    #[must_use]
    pub fn status(&self) -> IndexStatus {
        IndexStatus {
            roots: self.roots.clone(),
            files: u64::try_from(self.entries.len()).unwrap_or(u64::MAX),
            content_bytes: self.content_bytes,
            revision: self.revision.clone(),
        }
    }

    /// Returns indexed roots in stable insertion order.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    #[cfg(feature = "live")]
    pub(crate) fn event_roots(&self) -> &[PathBuf] {
        &self.event_roots
    }

    fn with_storage_exclusion(
        &self,
        scan_options: ScanOptions,
        storage_path: Option<&Path>,
    ) -> Result<ScanOptions> {
        let Some(storage_path) = storage_path else {
            return Ok(scan_options);
        };
        scan_options_with_storage_exclusion(&self.roots, scan_options, storage_path)
    }

    /// Returns the selected-file count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the index contains no selected files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the deterministic root/path/content revision.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

fn set_failure(slot: &Mutex<Option<Error>>, error: Error) {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn has_failure(slot: &Mutex<Option<Error>>) -> bool {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

fn take_failure(slot: &Mutex<Option<Error>>) -> Option<Error> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

fn validate_unique_entries(entries: &[IndexEntry], path: &Path) -> Result<()> {
    for pair in entries.windows(2) {
        if pair[0].key() >= pair[1].key() {
            return Err(Error::index(path, "entries are not strictly ordered"));
        }
    }
    Ok(())
}

fn entry_position(
    entries: &[IndexEntry],
    root_index: usize,
    path: &str,
) -> std::result::Result<usize, usize> {
    entries.binary_search_by(|entry| entry.key().cmp(&(root_index, path)))
}

fn content_bytes(entries: &[IndexEntry], path: &Path) -> Result<u64> {
    entries.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(u64::try_from(entry.content.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| Error::index(path, "content byte count overflow"))
    })
}

fn revision(roots: &[PathBuf], entries: &[IndexEntry]) -> String {
    hex(&revision_bytes(roots, entries))
}

fn revision_bytes(roots: &[PathBuf], entries: &[IndexEntry]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"weavatrix-search-index-revision-v1");
    for root in roots {
        let encoded = encode_path(root);
        digest.update(
            u64::try_from(encoded.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(encoded);
    }
    for entry in entries {
        digest.update(
            u64::try_from(entry.root_index)
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(
            u64::try_from(entry.path.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(entry.path.as_bytes());
        digest.update(entry.content_hash);
    }
    digest.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn scan_options_with_storage_exclusion(
    roots: &[PathBuf],
    mut scan_options: ScanOptions,
    storage_path: &Path,
) -> Result<ScanOptions> {
    let storage_path =
        std::path::absolute(storage_path).map_err(|source| Error::io(storage_path, source))?;
    let resolved_storage = storage_path.canonicalize().unwrap_or_else(|_| {
        storage_path.parent().map_or_else(
            || storage_path.clone(),
            |parent| {
                parent
                    .canonicalize()
                    .ok()
                    .and_then(|parent| {
                        storage_path
                            .file_name()
                            .map(|file_name| parent.join(file_name))
                    })
                    .unwrap_or_else(|| storage_path.clone())
            },
        )
    });
    for root in roots {
        let resolved_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let Ok(relative) = resolved_storage.strip_prefix(&resolved_root) else {
            continue;
        };
        let relative = relative.to_str().ok_or_else(|| {
            Error::index(
                &storage_path,
                "an index inside a root must have a UTF-8 relative path",
            )
        })?;
        let escaped = escape_override_literal(relative);
        scan_options.override_rules.extend([
            format!("!/{escaped}"),
            format!("!/{escaped}.lock"),
            format!("!/{escaped}.tmp.*"),
            format!("!/{escaped}.backup.*"),
        ]);
    }
    Ok(scan_options)
}

fn escape_override_literal(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.replace('\\', "/").chars() {
        if matches!(character, '\\' | '*' | '?' | '[' | ']' | '{' | '}' | ' ') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn trigram_bloom(bytes: &[u8]) -> [u64; BLOOM_WORDS] {
    let mut bloom = [0_u64; BLOOM_WORDS];
    for window in bytes.windows(3) {
        let trigram =
            (u32::from(window[0]) << 16) | (u32::from(window[1]) << 8) | u32::from(window[2]);
        let (first, second) = bloom_positions(trigram);
        bloom[first / 64] |= 1_u64 << (first % 64);
        bloom[second / 64] |= 1_u64 << (second % 64);
    }
    bloom
}

fn bloom_contains(bloom: &[u64; BLOOM_WORDS], trigram: u32) -> bool {
    let (first, second) = bloom_positions(trigram);
    (bloom[first / 64] & (1_u64 << (first % 64))) != 0
        && (bloom[second / 64] & (1_u64 << (second % 64))) != 0
}

fn bloom_positions(trigram: u32) -> (usize, usize) {
    const BITS: u64 = (BLOOM_WORDS * 64) as u64;
    let value = u64::from(trigram);
    let first = value.wrapping_mul(0x9e37_79b1_85eb_ca87) % BITS;
    let second = value.rotate_left(17).wrapping_mul(0xc2b2_ae3d_27d4_eb4f) % BITS;
    (
        usize::try_from(first).unwrap_or(0),
        usize::try_from(second).unwrap_or(0),
    )
}

struct IndexWriter {
    writer: BufWriter<File>,
    digest: Sha256,
    written: u64,
    limit: u64,
}

impl IndexWriter {
    fn new(file: File, limit: u64) -> Self {
        Self {
            writer: BufWriter::new(file),
            digest: Sha256::new(),
            written: 0,
            limit,
        }
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.written = self
            .written
            .checked_add(length)
            .ok_or_else(|| Error::index("<writer>", "serialized byte count overflow"))?;
        if self.written.saturating_add(CHECKSUM_BYTES) > self.limit {
            return Err(Error::index(
                "<writer>",
                format!("serialized index exceeds {} bytes", self.limit),
            ));
        }
        self.writer
            .write_all(bytes)
            .map_err(|source| Error::io("<writer>", source))?;
        self.digest.update(bytes);
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<()> {
        self.bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn length_prefixed(&mut self, bytes: &[u8], max: usize) -> Result<()> {
        if bytes.len() > max {
            return Err(Error::index(
                "<writer>",
                format!("path length {} exceeds {max}", bytes.len()),
            ));
        }
        self.u32(
            u32::try_from(bytes.len())
                .map_err(|_| Error::index("<writer>", "path length exceeds u32"))?,
        )?;
        self.bytes(bytes)
    }

    fn finish(mut self, path: &Path) -> Result<()> {
        let checksum = self.digest.finalize();
        self.writer
            .write_all(&checksum)
            .and_then(|()| self.writer.flush())
            .map_err(|source| Error::io(path, source))?;
        self.writer
            .get_ref()
            .sync_all()
            .map_err(|source| Error::io(path, source))
    }
}

struct IndexReader {
    reader: BufReader<File>,
    digest: Sha256,
    read: u64,
    limit: u64,
}

impl IndexReader {
    fn new(file: File, limit: u64) -> Self {
        Self {
            reader: BufReader::new(file),
            digest: Sha256::new(),
            read: 0,
            limit,
        }
    }

    fn bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        let length_u64 = u64::try_from(length).unwrap_or(u64::MAX);
        self.read = self
            .read
            .checked_add(length_u64)
            .ok_or_else(|| Error::index("<reader>", "serialized byte count overflow"))?;
        if self.read.saturating_add(CHECKSUM_BYTES) > self.limit {
            return Err(Error::index(
                "<reader>",
                format!("serialized index exceeds {} bytes", self.limit),
            ));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|error| Error::index("<reader>", format!("allocation failed: {error}")))?;
        bytes.resize(length, 0);
        self.reader
            .read_exact(&mut bytes)
            .map_err(|source| Error::io("<reader>", source))?;
        self.digest.update(&bytes);
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.bytes(N)?
            .try_into()
            .map_err(|_| Error::index("<reader>", "fixed-width field length mismatch"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn length_prefixed(&mut self, max: usize) -> Result<Vec<u8>> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| Error::index("<reader>", "path length does not fit usize"))?;
        if length > max {
            return Err(Error::index(
                "<reader>",
                format!("path length {length} exceeds {max}"),
            ));
        }
        self.bytes(length)
    }

    fn finish(mut self) -> Result<ChecksumResult> {
        let expected = {
            let mut bytes = [0_u8; 32];
            self.reader
                .read_exact(&mut bytes)
                .map_err(|source| Error::io("<reader>", source))?;
            bytes
        };
        let mut trailing = [0_u8; 1];
        let trailing_bytes = self
            .reader
            .read(&mut trailing)
            .map_err(|source| Error::io("<reader>", source))?;
        if trailing_bytes != 0 {
            return Err(Error::index("<reader>", "unexpected trailing bytes"));
        }
        let actual: [u8; 32] = self.digest.finalize().into();
        Ok(ChecksumResult {
            valid: actual == expected,
        })
    }
}

struct ChecksumResult {
    valid: bool,
}

struct IndexLock {
    path: PathBuf,
}

impl IndexLock {
    fn acquire(index_path: &Path) -> Result<Self> {
        let path = suffix_path(index_path, "lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    Error::index(index_path, "another writer holds the index lock")
                } else {
                    Error::io(&path, source)
                }
            })?;
        writeln!(file, "{}", std::process::id()).map_err(|source| Error::io(&path, source))?;
        file.sync_all().map_err(|source| Error::io(&path, source))?;
        Ok(Self { path })
    }
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn auxiliary_path(path: &Path, suffix: &str) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    suffix_path(
        path,
        &format!("{suffix}.{}.{}", std::process::id(), sequence),
    )
}

fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".");
    value.push(suffix);
    PathBuf::from(value)
}

fn replace_file(temp: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return fs::rename(temp, target).map_err(|source| Error::io(target, source));
    }
    let backup = auxiliary_path(target, "backup");
    fs::rename(target, &backup).map_err(|source| Error::io(target, source))?;
    match fs::rename(temp, target) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(source) => {
            let _ = fs::rename(&backup, target);
            Err(Error::io(target, source))
        }
    }
}

#[cfg(unix)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(unix)]
// Windows rejects odd UTF-16 payloads; keeping one fallible decoder signature
// makes the format reader identical on every platform.
#[allow(clippy::unnecessary_wraps)]
fn decode_path(bytes: &[u8]) -> std::result::Result<PathBuf, &'static str> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(windows)]
fn decode_path(bytes: &[u8]) -> std::result::Result<PathBuf, &'static str> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    if !bytes.len().is_multiple_of(2) {
        return Err("Windows root path has an odd byte length");
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}
